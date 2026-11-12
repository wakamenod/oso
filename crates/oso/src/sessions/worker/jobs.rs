//! What the sessions worker does with each job, apart from its thread and its clock: the
//! instants come from the caller, so the tests drive it with made-up ones.
//!
//! Must not depend on: `app`, `commands` or `dispatch`.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use oso_platform::file_meta::open_for_reading;

use super::{EVALUATE_EVERY, Job, Out, SessionKey};
use crate::sessions::engine::evaluate;
use crate::sessions::explain;
use crate::sessions::rules::RuleSet;
use crate::sessions::screen::Evidence;
use crate::sessions::state::Tracker;

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
}

/// The rules in force and the sessions.
#[derive(Debug)]
pub struct Core {
    rules: Arc<RuleSet>,
    sessions: HashMap<SessionKey, Session>,
}

impl Core {
    pub fn new(rules: Arc<RuleSet>) -> Self {
        Self {
            rules,
            sessions: HashMap::new(),
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
                        .map_or_else(Instant::now, |at| at + EVALUATE_EVERY)
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
                for session in self.sessions.values_mut() {
                    session.fresh |= session.evidence.is_some();
                    session.judged = None;
                }
                return Vec::new();
            }
            Job::Start(key) => {
                let session = Session {
                    tracker: Tracker::new(now),
                    evidence: None,
                    fresh: false,
                    judged: None,
                };
                self.sessions.insert(key, session);
                return Vec::new();
            }
            Job::Screen(key, evidence) => {
                if let Some(session) = self.sessions.get_mut(&key) {
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
        for (&key, session) in &mut self.sessions {
            let screen_due =
                session.fresh && session.judged.is_none_or(|at| now >= at + EVALUATE_EVERY);
            let tracker_due = session.tracker.due().is_some_and(|due| now >= due);
            if !(screen_due || tracker_due) {
                continue;
            }
            let Some(evidence) = &session.evidence else {
                continue;
            };
            let verdict = evaluate(&self.rules, evidence);
            session.fresh = false;
            session.judged = Some(now);
            if let Some(change) = session.tracker.verdict(verdict.target, now) {
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
        let shown = path.display().to_string();
        match read_screen(path) {
            Ok(text) => {
                let evidence = Evidence::from_text(&text);
                let verdict = evaluate(&self.rules, &evidence);
                Out::Explained(explain::text(&shown, &self.rules, &evidence, &verdict))
            }
            Err(why) => Out::Unexplained(format!("Cannot explain {shown}: {why}")),
        }
    }
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
