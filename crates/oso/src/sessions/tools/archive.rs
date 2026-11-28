//! The rules of an agent's `session_archive` (ADR 0179): which session a caller may archive,
//! when one that runs is idle enough to be stopped first, and what the question, the echo area
//! and the reply say.
//!
//! Must not depend on: the app, threads or the disk; the rules are plain data and functions.

use super::{Asks, CallerStanding, Tool, quoted};
use crate::sessions::registry::{self, Activity, Entry, Live};

/// What an archive knows of the session it would archive and of who asks (ADR 0179).
#[derive(Clone, Copy, Debug, Default)]
pub struct Archivable {
    /// It is the calling session.
    pub caller: bool,
    /// It is a space's orchestrator, current or earlier, as the record or its directory says.
    pub orchestrator: bool,
    /// The person pinned it.
    pub pinned: bool,
    /// It is archived already.
    pub archived: bool,
    /// The caller is the current orchestrator of the space it is in.
    pub of_callers_space: bool,
    /// The caller started it, as the record's parent says.
    pub callers_child: bool,
    /// How it runs, if it does.
    pub runs: Runs,
}

/// How a session to archive runs, as the UI thread knows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Runs {
    /// It does not run: stopped, failed or remembered.
    #[default]
    No,
    /// Its start thread runs.
    Starting,
    /// oso hung it up, and its process has not ended yet.
    Stopping,
    /// It runs as this process, with its screen's state settled (idle, done or ready for
    /// review) or not.
    Yes { pid: Option<u32>, settled: bool },
}

/// What an archive does, once allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archive {
    /// The row only: the session does not run.
    Row,
    /// Its `claude` is stopped first, once Claude Code's registry says that process is idle.
    StopFirst { pid: u32 },
}

/// Whether `session` may be archived by an agent, and how, or why not (ADR 0179): never the
/// caller, an orchestrator, a pinned or archived session; a current orchestrator archives its
/// space's sessions, any caller those it started; one that runs only when its screen is settled.
///
/// # Errors
///
/// Why it is refused, for the agent.
pub fn archivable(session: Archivable) -> Result<Archive, &'static str> {
    if session.caller {
        return Err("that is your own session");
    }
    if session.orchestrator {
        return Err("that is a space's orchestrator; only the person archives it");
    }
    if !session.of_callers_space && !session.callers_child {
        return Err(
            "a session archives the sessions it started, and a space's orchestrator its space's sessions",
        );
    }
    if session.archived {
        return Err("that session is archived already");
    }
    if session.pinned {
        return Err("the person pinned that session; only the person archives it");
    }
    match session.runs {
        Runs::No => Ok(Archive::Row),
        Runs::Starting => Err("that session is still starting"),
        Runs::Stopping => Err("that session is stopping; try again once it has stopped"),
        Runs::Yes { settled: false, .. } => Err(
            "that session is not idle, done or ready for review: it may still work; only the person stops it",
        ),
        Runs::Yes {
            pid: None,
            settled: true,
        } => Err("oso does not know that session's process; only the person stops it"),
        Runs::Yes {
            pid: Some(pid),
            settled: true,
        } => Ok(Archive::StopFirst { pid }),
    }
}

/// How an archive's call asks, as `archive` does: the row alone asks nothing, as the person's
/// `A` undoes it; a stop asks as `tool`'s table entry says.
pub fn archive_asks(archive: Archive, tool: &Tool) -> Asks {
    match archive {
        Archive::Row => Asks::No,
        Archive::StopFirst { .. } => tool.asks,
    }
}

/// Whether an archive's stop goes without asking (ADR 0179): as the person's configuration
/// lets a start, from a session the person started, `caller`. No project's trust is
/// asked, as a stop runs nothing in a project, and no start limit counts it.
pub fn stops_without_asking(caller: CallerStanding) -> bool {
    caller.goes_unasked()
}

/// What Claude Code's registry says of process `pid` that keeps it from being stopped, as check
/// 4 of a worktree's removal reads it (ADR 0121): its entry, read once, whose process is `live`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotIdle {
    /// No entry for it could be read.
    NoEntry,
    /// Its entry may be another process's, or the system does not say when it started.
    Unverified,
    /// A shell it runs in the background still runs, which the registry calls `shell`.
    Shell,
    /// Any other status, such as `busy`.
    Busy,
}

impl NotIdle {
    /// Why the archive is refused, for the agent.
    pub fn why(self) -> &'static str {
        match self {
            Self::NoEntry => "Claude Code's registry has no entry for that session's process",
            Self::Unverified => {
                "Claude Code's registry entry for that session cannot be matched to its process"
            }
            Self::Shell => {
                "a background shell of that session still runs (Claude Code's registry says shell); it ends it, or the person stops the session"
            }
            Self::Busy => "Claude Code's registry says that session is busy",
        }
    }
}

/// Whether process `pid`'s registry `entry`, if one was read, says it is idle: the entry is
/// its process's ([`Live::Yes`]) and its status is `idle`. Every other answer keeps it running,
/// as check 4 keeps a worktree.
///
/// # Errors
///
/// What keeps it from being idle.
pub fn registry_idle(pid: u32, entry: Option<(&Entry, Live)>) -> Result<(), NotIdle> {
    let Some((entry, live)) = entry.filter(|(entry, _)| entry.pid == pid) else {
        return Err(NotIdle::NoEntry);
    };
    match registry::activity(entry, live) {
        Activity::Idle => Ok(()),
        Activity::Shell => Err(NotIdle::Shell),
        Activity::Busy => Err(NotIdle::Busy),
        Activity::Unverified => Err(NotIdle::Unverified),
    }
}

/// The question of a `session_archive` that stops session `name`, in `directory`, first: two
/// sessions may share a name, never a name and a directory.
pub fn archive_question(caller: &str, name: &str, directory: &str) -> String {
    format!(
        "let session {} stop idle session {} in {} and archive it? Its worktree is kept.",
        quoted(caller),
        quoted(name),
        quoted(directory)
    )
}

/// What the echo area says of an archive by agent `caller` of session `name`: whether it was
/// `stopped` first, and whether that went `unasked`, as the configuration allows.
pub fn archived_said(caller: &str, name: &str, stopped: bool, unasked: bool) -> String {
    let how = match (stopped, unasked) {
        (false, _) => "",
        (true, false) => ", stopping it first",
        (true, true) => ", stopping it first without asking",
    };
    format!(
        "session {} archived session {}{how}; A under Archived in *Spaces* brings it back",
        quoted(caller),
        quoted(name)
    )
}

#[cfg(test)]
mod tests;
