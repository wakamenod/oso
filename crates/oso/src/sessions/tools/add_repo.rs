//! The rules of an orchestrator's `space_add_repo` (ADR 0161): whether its space takes the
//! repository, what the person's question and the echo area say, and what the reply tells the
//! agent of `/add-dir`.
//!
//! Must not depend on: the app or threads; the rules are plain data and functions.

use super::quoted;
use crate::sessions::spaces::stored::MAX_MEMBERS;

/// Whether a space takes one more repository, or why not: it is open, does not have it, and
/// holds fewer than [`MAX_MEMBERS`].
///
/// # Errors
///
/// Why not, for the agent.
pub fn addable(open: bool, has_it: bool, members: usize) -> Result<(), String> {
    if !open {
        return Err("your space is closed or gone".into());
    }
    if has_it {
        return Err("your space has that repository already".into());
    }
    if members >= MAX_MEMBERS {
        return Err(format!("a space holds at most {MAX_MEMBERS} repositories"));
    }
    Ok(())
}

/// The question of `space_add_repo`: `caller` adds the repository at `path` to space `space`;
/// one oso does not trust says it joins restricted.
pub fn add_repo_question(caller: &str, path: &str, space: &str, trusted: bool) -> String {
    let restricted = if trusted {
        ""
    } else {
        " oso does not trust it: it joins restricted, with no worktrees, workers or --add-dir \
         until you trust it."
    };
    format!(
        "let session {} add repository {} to space {}?{restricted}",
        quoted(caller),
        quoted(path),
        quoted(space)
    )
}

/// What the echo area says of an add: `caller` added `path` to `space`, `unasked` when it went
/// with no question.
pub fn add_repo_said(caller: &str, path: &str, space: &str, unasked: bool) -> String {
    let how = if unasked { ", without asking" } else { "" };
    format!(
        "session {} added repository {} to space {}{how}",
        quoted(caller),
        quoted(path),
        quoted(space)
    )
}

/// Whether the orchestrator that added a repository can read it yet (Q5), as oso gives it one
/// it was not given, asking no one (ADR 0185).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddDir {
    /// oso types `/add-dir PATH` into its input and sends it, once its turn ends and its input
    /// box is empty.
    Queued,
    /// Its `claude` was given it with `--add-dir` already.
    Given,
    /// It is restricted until the person trusts it.
    WaitsForTrust,
    /// oso cannot type `/add-dir` for this path, as it is no plain line.
    NotQueued,
    /// The orchestrator runs natively, where `/add-dir` is no command: oso resumes it with the
    /// repository given with `--add-dir`, once its turn ends (ADR 0176, part O-2).
    ResumeQueued,
    /// It runs natively, and oso cannot resume it with this path now: its next resume is
    /// given it.
    NextResume,
}

impl AddDir {
    /// The reply's `add_dir`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Given => "given",
            Self::WaitsForTrust => "waits for trust",
            Self::NotQueued => "not queued",
            Self::ResumeQueued => "resume queued",
            Self::NextResume => "next resume",
        }
    }

    /// What the reply tells the agent of the repository at `path`, a `native` orchestrator's
    /// resume said in place of the typed command (ADR 0176, part O-2).
    pub fn note(self, path: &str, native: bool) -> String {
        match self {
            Self::Queued => format!(
                "oso types /add-dir {path} into your input and sends it once this turn ends and \
                 your input box is empty: you can read it from then. Workers you start with \
                 this repo can use it now."
            ),
            Self::Given => "your claude was given it with --add-dir already.".to_owned(),
            Self::WaitsForTrust if native => format!(
                "oso does not trust it: it is restricted, with no worktrees, workers or \
                 --add-dir, until the person trusts it with trust-project in oso; oso then \
                 resumes you with {path} given with --add-dir once your turn ends."
            ),
            Self::WaitsForTrust => format!(
                "oso does not trust it: it is restricted, with no worktrees, workers or \
                 --add-dir, until the person trusts it with trust-project in oso; oso then \
                 types /add-dir {path} into your input and sends it once your turn ends."
            ),
            Self::NotQueued => format!(
                "oso cannot type /add-dir for this path: ask the person to type /add-dir {path} \
                 in your input. Workers you start with this repo can use it now."
            ),
            Self::ResumeQueued => format!(
                "you run as a native session, which takes no /add-dir: oso resumes you with \
                 {path} given with --add-dir once this turn ends, your conversation kept. You \
                 cannot read it until then. Workers you start with this repo can use it now."
            ),
            Self::NextResume => format!(
                "you run as a native session, which takes no /add-dir, and oso cannot resume \
                 you with this path now: oso gives you {path} with --add-dir when you are \
                 resumed next. Workers you start with this repo can use it now."
            ),
        }
    }
}

/// What the reply says once a repository is added: restricted when not `trusted`, else
/// `given` already, else whether oso `queued` the typed `/add-dir`, or for a `native`
/// orchestrator the resume with it (ADR 0176, part O-2; ADR 0185).
pub fn add_dir(trusted: bool, given: bool, queued: bool, native: bool) -> AddDir {
    match (trusted, given, queued) {
        (false, _, _) => AddDir::WaitsForTrust,
        (true, true, _) => AddDir::Given,
        (true, false, true) if native => AddDir::ResumeQueued,
        (true, false, false) if native => AddDir::NextResume,
        (true, false, true) => AddDir::Queued,
        (true, false, false) => AddDir::NotQueued,
    }
}

#[cfg(test)]
mod tests;
