//! What the sessions worker does with each job, apart from its thread and its clock: the
//! instants come from the caller, so the tests drive it with made-up ones.
//!
//! Must not depend on: `app`, `commands` or `dispatch`.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_platform::file_meta::open_for_reading;

use super::{DROP_AFTER, EVALUATE_EVERY, Job, Out, SLOW_AFTER, SessionKey};
use crate::sessions::engine::{Fallback, evaluate, evaluate_within};
use crate::sessions::explain;
use crate::sessions::rules::{BUDGET, RuleSet, Source};
use crate::sessions::screen::Evidence;
use crate::sessions::state::{Target, Tracker};

/// The largest saved screen `session-explain-file` reads.
pub const MAX_EXPLAIN_BYTES: u64 = 256 * 1024;

/// One hosted session on the worker.
#[derive(Debug)]
struct Session {
    tracker: Tracker,
    /// The newest look at its screen.
    evidence: Option<Evidence>,
    /// Whether that look has not been judged yet.
    fresh: bool,
    /// When its screen was last judged.
    judged: Option<Instant>,
    /// The last verdict, which a wake-up of the debounce or the grace period gives again: a
    /// screen that has not changed is not judged twice.
    last: Option<Target>,
    /// How many looks in a row ran out of time: each doubles the wait before the next.
    late: u32,
}

impl Session {
    /// The least time between two judgements of its screen: [`EVALUATE_EVERY`], doubled for
    /// each look in a row that ran out of time, at most [`MAX_BACKOFF`].
    fn every(&self) -> Duration {
        EVALUATE_EVERY
            .saturating_mul(1 << self.late.min(16))
            .min(MAX_BACKOFF)
    }
}

/// The longest wait between two judgements of a session whose rules keep running out of time.
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// The rules in force and the sessions.
#[derive(Debug)]
pub struct Core {
    rules: Arc<RuleSet>,
    sessions: HashMap<SessionKey, Session>,
    /// Whether these rules were said to take too long.
    slow_said: bool,
    /// How long the rules may take over one look.
    budget: Duration,
    /// The rule that was the slowest of the last late looks, and how many in a row.
    streak: Option<(String, u32)>,
    /// The local rules no longer used, until the rules load again.
    dropped: HashSet<String>,
}

impl Core {
    pub fn new(rules: Arc<RuleSet>) -> Self {
        Self {
            rules,
            sessions: HashMap::new(),
            slow_said: false,
            budget: BUDGET,
            streak: None,
            dropped: HashSet::new(),
        }
    }

    /// The same, with the rules' `budget` over one look.
    #[cfg(test)]
    pub fn with_budget(rules: Arc<RuleSet>, budget: Duration) -> Self {
        Self {
            budget,
            ..Self::new(rules)
        }
    }

    /// When the worker must wake with no job: the soonest a session's new screen may be judged,
    /// or its tracker waits for. `None` while nothing waits.
    pub fn due(&self) -> Option<Instant> {
        self.sessions
            .values()
            .filter_map(|session| {
                let screen = session.fresh.then(|| {
                    session
                        .judged
                        .map_or_else(Instant::now, |at| at + session.every())
                });
                match (screen, session.tracker.due()) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            })
            .min()
    }

    /// Does `job` at `now`.
    pub fn handle(&mut self, job: Job, now: Instant) -> Vec<Out> {
        let (key, change) = match job {
            Job::Rules(rules) => {
                self.rules = rules;
                self.slow_said = false;
                self.streak = None;
                self.dropped.clear();
                for session in self.sessions.values_mut() {
                    session.fresh |= session.evidence.is_some();
                    session.judged = None;
                    session.late = 0;
                }
                return Vec::new();
            }
            Job::Start(key) => {
                let session = Session {
                    tracker: Tracker::new(now),
                    evidence: None,
                    fresh: false,
                    judged: None,
                    last: None,
                    late: 0,
                };
                self.sessions.insert(key, session);
                return Vec::new();
            }
            Job::Screen(key, evidence) => {
                // A look that found what the last one did changes nothing to judge.
                if let Some(session) = self.sessions.get_mut(&key)
                    && session.evidence.as_ref() != Some(&evidence)
                {
                    session.evidence = Some(evidence);
                    session.fresh = true;
                }
                return Vec::new();
            }
            Job::Forget(key) => {
                self.sessions.remove(&key);
                return Vec::new();
            }
            Job::ExplainFile(path) => return vec![self.explain_file(&path)],
            Job::Exited(key, exit) => (key, self.tracker(key, |t| t.exited(exit, now))),
            Job::Seen(key) => (key, self.tracker(key, |t| t.seen(now))),
            Job::Changes(key, yes) => (key, self.tracker(key, |t| t.changes(yes, now))),
            Job::ReviewOpened(key) => (key, self.tracker(key, |t| t.review_opened(now))),
        };
        change
            .map(|change| Out::Changed(key, change))
            .into_iter()
            .collect()
    }

    /// Judges each session whose new screen is due, or whose tracker waits, at `now`.
    pub fn tick(&mut self, now: Instant) -> Vec<Out> {
        let mut found = Vec::new();
        let Self {
            rules,
            sessions,
            slow_said,
            budget,
            streak,
            dropped,
        } = self;
        for (&key, session) in sessions {
            let screen_due =
                session.fresh && session.judged.is_none_or(|at| now >= at + session.every());
            let tracker_due = session.tracker.due().is_some_and(|due| now >= due);
            if !(screen_due || tracker_due) {
                continue;
            }
            let target = if screen_due {
                let Some(evidence) = &session.evidence else {
                    continue;
                };
                // Marked first: a panic while judging must not leave it due at once.
                session.fresh = false;
                session.judged = Some(now);
                let verdict = evaluate_within(rules, evidence, *budget, dropped);
                session.judged = Some(Instant::now().max(now));
                if let Err(Fallback::TooSlow { slowest }) = &verdict.by {
                    session.late = session.late.saturating_add(1);
                    // Not judged after all: the same screen is judged again once the longer
                    // wait is over, so that a dialog that stays is not hidden until it changes.
                    session.fresh = true;
                    if session.late >= SLOW_AFTER && !*slow_said {
                        *slow_said = true;
                        found.push(Out::Slow(slowest.clone()));
                    }
                    if drops(streak, slowest, rules) && dropped.insert(slowest.clone()) {
                        *streak = None;
                        found.push(Out::Dropped(slowest.clone()));
                    }
                } else {
                    session.late = 0;
                    *streak = None;
                }
                session.last = Some(verdict.target);
                verdict.target
            } else {
                let Some(last) = session.last else {
                    continue;
                };
                last
            };
            if let Some(change) = session.tracker.verdict(target, now) {
                found.push(Out::Changed(key, change));
            }
        }
        found
    }

    fn tracker<R>(
        &mut self,
        key: SessionKey,
        work: impl FnOnce(&mut Tracker) -> Option<R>,
    ) -> Option<R> {
        self.sessions
            .get_mut(&key)
            .and_then(|session| work(&mut session.tracker))
    }

    /// Reads the saved screen at `path` and explains it with the rules in force.
    fn explain_file(&self, path: &Path) -> Out {
        match read_screen(path) {
            Ok(text) => {
                let shown = path.display().to_string();
                let evidence = Evidence::from_text(&text);
                let verdict = evaluate(&self.rules, &evidence);
                Out::Explained(explain::text(&shown, &self.rules, &evidence, &verdict))
            }
            Err(why) => Out::Unexplained(format!("Cannot explain that screen file: {why}")),
        }
    }
}

/// Counts a late look whose slowest rule was `slowest`, and whether that local rule has now
/// been the slowest of [`DROP_AFTER`] late looks in a row.
fn drops(streak: &mut Option<(String, u32)>, slowest: &str, rules: &RuleSet) -> bool {
    let count = match streak {
        Some((name, count)) if name == slowest => {
            *count = count.saturating_add(1);
            *count
        }
        _ => {
            *streak = Some((slowest.to_owned(), 1));
            1
        }
    };
    count >= DROP_AFTER
        && rules
            .rules()
            .iter()
            .any(|rule| rule.name == slowest && rule.source == Source::Local)
}

/// The text of the regular file at `path`, of at most [`MAX_EXPLAIN_BYTES`], invalid UTF-8
/// replaced.
fn read_screen(path: &Path) -> Result<String, String> {
    let (file, metadata) = match open_for_reading(path) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Err("it is not a regular file".to_owned()),
        Err(e) => return Err(e.to_string()),
    };
    let too_large = || format!("it is larger than {} KiB", MAX_EXPLAIN_BYTES / 1024);
    if metadata.len() > MAX_EXPLAIN_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(MAX_EXPLAIN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_EXPLAIN_BYTES {
        return Err(too_large());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests;
