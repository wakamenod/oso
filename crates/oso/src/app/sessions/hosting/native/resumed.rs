//! What a native session's resume says of how it went (ADR 0165, "Worktrees' resumes", the
//! author's Q5): its transcript's row, its row of `*Spaces*` while it stays stopped, and, once
//! every resume of an `oso-restart` has reported, one line for those that did not.
//!
//! Must not depend on: key presses or how a frame is drawn. It decides nothing but what is
//! said, and that a refused session's conversation may go on elsewhere.
//!
//! - A restart's resume says "Resumed after oso-restart." once its process started, not as it
//!   was ordered: the start thread may still refuse it.
//! - That line waits until the agent channel, which the resumes ask for, has said that it is on,
//!   as that would take its place in the echo area at once: [`CHANNEL_TIME`] at most.
//! - A refusal's reason is the start thread's or the judge's; it names no path a repository
//!   wrote. It is kept in memory alone, until the session runs.

use std::time::{Duration, Instant};

use super::restarting::Resuming;
use crate::app::App;
use crate::app::sessions::spaces::printable;
use crate::channel::protocol::mode::PermissionMode;
use crate::sessions::recap::clean;
use crate::sessions::start::NOT_STARTED;
use crate::sessions::worker::SessionKey;

/// The names the line for a restart's refused resumes gives.
const NAMED: usize = 3;

/// How long that line waits for the agent channel, whose thread may hang.
const CHANNEL_TIME: Duration = Duration::from_secs(5);

/// What a resume under way says of itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ResumeFor {
    /// "Resumed." as it is ordered, or a repository added once it runs (ADR 0176).
    #[default]
    Asked,
    /// The resume of a session the instance before ran, in the mode carried, if any.
    Restarted(Option<PermissionMode>),
    /// The resume of a session ended for a restart that stayed here.
    Stayed,
}

impl ResumeFor {
    /// The row once its process started, if it has one.
    fn resumed(self) -> Option<&'static str> {
        match self {
            ResumeFor::Asked => None,
            ResumeFor::Restarted(_) => Some("Resumed after oso-restart."),
            ResumeFor::Stayed => Some("oso-restart stayed here: resumed."),
        }
    }

    /// The row of a resume refused as `why` says.
    fn refused(self, why: &str) -> String {
        match self {
            ResumeFor::Asked => format!("It did not resume: {why}"),
            ResumeFor::Restarted(None) => format!("It did not resume after oso-restart: {why}"),
            ResumeFor::Restarted(Some(mode)) => format!(
                "It did not resume after oso-restart: {why}. Its next resume starts in {}.",
                mode.name()
            ),
            ResumeFor::Stayed => format!("oso-restart stayed here; it did not resume: {why}"),
        }
    }
}

/// Why a resume whose conversation is live, in process `pid` if the registry says, is refused.
pub(in crate::app::sessions::hosting) fn runs_elsewhere(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("its conversation runs already as process {pid}"),
        None => "its conversation may run already: oso cannot tell from Claude Code's registry"
            .to_owned(),
    }
}

/// How each reason that is too long for a row of `*Spaces*`, which a recap's length cuts, is
/// said there, by how the reason starts.
const SHORT: [(&str, &str); 8] = [
    (
        "its repository is not trusted now",
        "repository not trusted; trust-project, then session-resume",
    ),
    (
        "its directory is neither a trusted",
        "not a trusted repository nor a worktree of one",
    ),
    (
        "its project is not trusted now",
        "project not trusted now; session-resume once it is",
    ),
    (
        "its project's trust is not known",
        "the record of trusted projects is not read",
    ),
    (
        "the record of trusted projects does not hold",
        "not the directory trusted; trust-project if it is yours",
    ),
    (
        "its project's root is another directory",
        "not the directory trusted; trust-project if it is yours",
    ),
    (
        "its space closed",
        "its space closed, or is no longer its own",
    ),
    (
        "its conversation may run already",
        "its conversation may run already elsewhere",
    ),
];

/// What the row of `*Spaces*` of session `name` says of a resume refused as `why`: in short
/// words that a recap's length does not cut, or else its first clause.
fn list_row(why: &str, name: &str) -> String {
    // The row names its session.
    let of = format!("Session {name}");
    let why = match why.strip_prefix(&of) {
        Some(rest) => match rest.strip_prefix("'s ") {
            Some(rest) => format!("its {rest}"),
            None => format!("it{rest}"),
        },
        None => why.to_owned(),
    };
    let short = (SHORT.iter())
        .find(|(starts, _)| why.starts_with(starts))
        .map_or(why.as_str(), |(_, short)| short);
    let said = |why: &str| clean(&format!("not resumed: {why}"));
    let whole = said(short);
    match short.split_once(": ") {
        Some((first, _)) if whole.ends_with('…') => said(first),
        _ => whole,
    }
}

/// What the echo area says of the sessions `left` stopped of those a restart resumes: `None`
/// for none.
fn left_stopped(left: &[String]) -> Option<String> {
    let named = left.iter().take(NAMED).cloned().collect::<Vec<_>>();
    let mut names = named.join(", ");
    if left.len() > NAMED {
        names.push_str(&format!(" and {} more", left.len() - NAMED));
    }
    match left.len() {
        0 => None,
        1 => Some(format!(
            "oso-restart left 1 session stopped: {names}; its transcript says why"
        )),
        n => Some(format!(
            "oso-restart left {n} sessions stopped: {names}; each transcript says why"
        )),
    }
}

impl Resuming {
    /// Those a restart resumes, once, when each has reported and the agent channel no longer
    /// comes on, or has for [`CHANNEL_TIME`]: `starts` says whether one still starts,
    /// `channel_comes` whether the channel may come on still.
    fn over(
        &mut self,
        starts: impl Fn(&SessionKey) -> bool,
        channel_comes: bool,
        now: Instant,
    ) -> Option<Vec<SessionKey>> {
        if self.asked.is_empty() || self.asked.iter().any(starts) {
            self.reported = None;
            return None;
        }
        let since = *self.reported.get_or_insert(now);
        if channel_comes && now.duration_since(since) < CHANNEL_TIME {
            return None;
        }
        self.reported = None;
        Some(std::mem::take(&mut self.asked))
    }

    /// When what waits for the agent channel is said without it.
    fn deadline(&self) -> Option<Instant> {
        Some(self.reported? + CHANNEL_TIME)
    }
}

impl App {
    /// Native session `key` is about to be resumed for `what`.
    pub(super) fn native_resumes_for(&mut self, key: SessionKey, what: ResumeFor) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.resuming_for = what;
        }
    }

    /// Whether native session `key`'s resume says so once its process started.
    pub(super) fn resume_says_on_start(&self, key: SessionKey) -> bool {
        (self.hosting.natives.map.get(&key))
            .is_some_and(|native| native.resuming_for.resumed().is_some())
    }

    /// Native session `key`'s process started: its resume's row, and nothing is left to say of
    /// one refused before.
    pub(super) fn native_resumed(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        native.not_resumed = None;
        if let Some(row) = std::mem::take(&mut native.resuming_for).resumed() {
            self.native_note_row(key, row);
        }
        self.sessions_touched();
    }

    /// Native session `key` did not resume, as `why` says: its transcript says so, and its row
    /// of `*Spaces*` until it runs.
    pub(super) fn native_resume_failed(&mut self, key: SessionKey, why: &str) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let why = why.strip_prefix(NOT_STARTED).unwrap_or(why);
        let row = std::mem::take(&mut native.resuming_for).refused(why);
        let name = self.hosting.table.get(key).map_or("", |s| s.name.as_str());
        native.not_resumed = Some(list_row(why, name));
        // Stopped now, it may be resumed elsewhere (part 5b).
        native.may_have_gone_on = true;
        self.native_note_row(key, &row);
        self.sessions_touched();
    }

    /// Whether start `key` is a restart's resume, whose refusal is said with the others'.
    pub(in crate::app::sessions::hosting) fn restart_resumes(&self, key: SessionKey) -> bool {
        self.hosting.natives.resuming.asked.contains(&key)
    }

    /// After each event, and before the loop waits: once none of the sessions a restart resumes
    /// is starting, and the agent channel is on if it comes on, those left stopped are said,
    /// once. One that went meanwhile is not.
    pub(in crate::app) fn sync_restart_resumes(&mut self) {
        // With none, nothing is looked at.
        if self.hosting.natives.resuming.asked.is_empty() {
            return;
        }
        let channel_comes = self.restart_holds() || self.agent_channel_starting();
        let hosting = &mut self.hosting;
        let starts = |key: &SessionKey| hosting.starting.contains_key(key);
        let resuming = &mut hosting.natives.resuming;
        let Some(asked) = resuming.over(starts, channel_comes, Instant::now()) else {
            return;
        };
        let left: Vec<String> = (asked.iter())
            .filter(|key| {
                (hosting.natives.map.get(key)).is_some_and(|native| native.not_resumed.is_some())
            })
            .filter_map(|key| Some(printable(&hosting.table.get(*key)?.name)))
            .collect();
        if let Some(said) = left_stopped(&left) {
            self.editor.message(said);
            self.request_redraw();
        }
    }
}

impl super::NativeSessions {
    /// When the loop wakes to say what a restart left stopped, if that waits.
    pub(in crate::app) fn restart_resumes_deadline(&self) -> Option<Instant> {
        self.resuming.deadline()
    }

    /// Why session `key` did not resume, as its row of `*Spaces*` says it, until it runs.
    pub(in crate::app) fn left_stopped_why(&self, key: SessionKey) -> Option<&str> {
        self.map.get(&key)?.not_resumed.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::super::resume::not_trusted;
    use super::{
        CHANNEL_TIME, Duration, Instant, ResumeFor, Resuming, SessionKey, left_stopped, list_row,
        runs_elsewhere,
    };
    use crate::channel::protocol::mode::PermissionMode;
    use crate::sessions::start::trust::{NOT_ONE, NOT_RECORDED, NotTrusted, UNLISTED};
    use crate::sessions::start::{NOT_STARTED, ROOT_REPLACED, space_gate};

    #[test]
    fn a_resumes_row_says_how_it_went_and_why() {
        let why = NOT_ONE;
        assert_eq!(ResumeFor::Asked.resumed(), None);
        assert_eq!(
            ResumeFor::Restarted(None).resumed(),
            Some("Resumed after oso-restart.")
        );
        assert_eq!(
            ResumeFor::Stayed.resumed(),
            Some("oso-restart stayed here: resumed.")
        );
        assert_eq!(
            ResumeFor::Asked.refused(why),
            format!("It did not resume: {why}")
        );
        assert_eq!(
            ResumeFor::Restarted(None).refused(why),
            format!("It did not resume after oso-restart: {why}")
        );
        assert_eq!(
            ResumeFor::Restarted(Some(PermissionMode::Plan)).refused(why),
            format!("It did not resume after oso-restart: {why}. Its next resume starts in plan.")
        );
        assert_eq!(
            ResumeFor::Stayed.refused(why),
            format!("oso-restart stayed here; it did not resume: {why}")
        );
        assert_eq!(
            runs_elsewhere(Some(7)),
            "its conversation runs already as process 7"
        );
        assert!(runs_elsewhere(None).contains("cannot tell"));
    }

    #[test]
    fn each_reasons_row_of_the_list_is_whole() {
        let long = "a-session-with-a-name-as-long-as-a-branch-may-be-in-a-worktree";
        let reasons = [
            (UNLISTED.to_owned(), "trust-project, then session-resume"),
            (NOT_ONE.to_owned(), "nor a worktree of one"),
            (NOT_RECORDED.to_owned(), "trust-project if it is yours"),
            (ROOT_REPLACED.to_owned(), "trust-project if it is yours"),
            (space_gate::TAKEN_BACK.to_owned(), "its space closed, or"),
            (
                format!("{NOT_STARTED}its project is no longer trusted"),
                "its project is no longer trusted",
            ),
            (
                not_trusted(long, NotTrusted::Untrusted),
                "session-resume once it is",
            ),
            (not_trusted(long, NotTrusted::NoRecord), "is not read"),
            (runs_elsewhere(Some(4_294_967)), "as process 4294967"),
            (runs_elsewhere(None), "may run already elsewhere"),
            (
                "The session's directory is not there any more".to_owned(),
                "not there any more",
            ),
        ];
        for (why, ends) in reasons {
            let why = why.strip_prefix(NOT_STARTED).unwrap_or(&why);
            let row = list_row(why, long);
            assert!(row.starts_with("not resumed: "), "{row}");
            assert!(row.contains(ends) && !row.ends_with('…'), "{row}");
            assert!(!row.contains(long), "its row names it: {row}");
        }
        // Below its root, which no short words are for, without what else to do.
        let below = list_row(&not_trusted("wa", NotTrusted::NotAtRoot), "wa");
        assert_eq!(below, "not resumed: it runs below its project's root");
    }

    /// A restart's resumes of two sessions, and their keys.
    fn resuming() -> (Resuming, Vec<SessionKey>) {
        let asked = vec![1, 2];
        let mut resuming = Resuming::default();
        resuming.asked.clone_from(&asked);
        (resuming, asked)
    }

    #[test]
    fn what_a_restart_left_stopped_is_said_once_after_the_agent_channel() {
        let (mut resuming, asked) = resuming();
        let now = Instant::now();
        let none = |_: &SessionKey| false;
        // One still starts, whether the channel comes on or not.
        assert_eq!(resuming.over(|key| *key == asked[1], false, now), None);
        assert_eq!(resuming.over(|key| *key == asked[1], true, now), None);
        assert_eq!(resuming.deadline(), None);
        // Each reported; the restart is not over, then the channel starts.
        assert_eq!(resuming.over(none, true, now), None);
        assert_eq!(resuming.over(none, true, now), None);
        assert_eq!(resuming.deadline(), Some(now + CHANNEL_TIME));
        // On, or failed, or off again: it comes on no more.
        assert_eq!(resuming.over(none, false, now), Some(asked));
        assert_eq!(resuming.over(none, false, now), None);
        assert_eq!(resuming.deadline(), None);
    }

    #[test]
    fn what_a_restart_left_stopped_is_said_at_once_if_no_channel_comes_on() {
        let (mut resuming, asked) = resuming();
        assert_eq!(resuming.over(|_| false, false, Instant::now()), Some(asked));
    }

    #[test]
    fn what_a_restart_left_stopped_is_said_though_the_agent_channel_never_reports() {
        let (mut resuming, asked) = resuming();
        let now = Instant::now();
        let before = now + CHANNEL_TIME.saturating_sub(Duration::from_millis(1));
        assert_eq!(resuming.over(|_| false, true, now), None);
        assert_eq!(resuming.over(|_| false, true, before), None);
        let due = resuming.deadline().unwrap();
        assert_eq!(resuming.over(|_| false, true, due), Some(asked));
        assert_eq!(resuming.deadline(), None);
    }

    #[test]
    fn what_a_restart_left_stopped_is_said_in_one_line() {
        let names = |n: usize| (0..n).map(|i| format!("s{i}")).collect::<Vec<_>>();
        assert_eq!(left_stopped(&names(0)), None);
        assert_eq!(
            left_stopped(&names(1)).unwrap(),
            "oso-restart left 1 session stopped: s0; its transcript says why"
        );
        assert_eq!(
            left_stopped(&names(3)).unwrap(),
            "oso-restart left 3 sessions stopped: s0, s1, s2; each transcript says why"
        );
        assert_eq!(
            left_stopped(&names(5)).unwrap(),
            "oso-restart left 5 sessions stopped: s0, s1, s2 and 2 more; each transcript says why"
        );
    }
}
