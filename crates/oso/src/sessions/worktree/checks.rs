//! What is known of one linked worktree, and the five checks `worktree-remove` runs before it
//! removes one, in ADR 0121's order: a linked worktree of this repository; nothing uncommitted
//! or untracked; no unsaved buffer or unsent brief; no busy session; its branch merged.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread; [`look`] runs on a
//! worktree job's thread, and what only the UI thread knows (buffers, hosted sessions) comes
//! with the job as [`Held`].
//!
//! A check that cannot be run refuses: a worktree is removed only when every check holds.

use std::path::{Path, PathBuf};

use super::facts::RepoFacts;
use super::git::{Gh, MAX_LISTING, Repo, run, said};
use super::merged::{self, Local, Merged};
use super::porcelain::{self, Listed, Status};
use crate::review::git::Git;

/// The most names of ignored files and directories kept, for the removal's question.
pub const MAX_IGNORED: usize = 1_000;

/// What the UI thread holds about a worktree when a job is asked for: its own state, which
/// the job cannot read.
#[derive(Clone, Debug, Default)]
pub struct Held {
    /// How many oso buffers under it have unsaved changes.
    pub unsaved: usize,
    /// Whether a `*Brief*` for it is not sent yet.
    pub unsent: bool,
    /// The hosted sessions in it that count as busy (see [`busy`]), by name.
    pub busy: Vec<String>,
}

/// A hosted session that runs, as the UI thread sees it when a job is asked for. A failed or
/// stopped session is never busy, so none is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedSession {
    pub directory: PathBuf,
    pub name: String,
    /// Its `claude`'s process id, while it runs.
    pub pid: Option<u32>,
    /// The state its screen gives is idle or done.
    pub settled: bool,
}

/// Whether a hosted session that runs is busy, for check 4 (the author's choice, ADR 0121):
/// unless its screen's state is idle or done *and* Claude Code's registry says `idle` for its
/// process (`registry_idle`). Each half alone can miss a turn or a background task.
pub fn busy(session: &HostedSession, registry_idle: bool) -> bool {
    !(session.settled && registry_idle)
}

/// A check that does not hold, by its number in ADR 0121's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// A check that could not be run, and why: it refuses as a failed one does.
    Unknown(String),
    /// 1: not a linked worktree of this repository, or one git cannot remove now.
    NotLinked(String),
    /// 2: changes not committed.
    Dirty(Status),
    /// 3: oso buffers under it with unsaved changes.
    Unsaved(usize),
    /// 3: a brief for it not sent.
    Unsent,
    /// 4: these hosted sessions in it are busy.
    Busy(Vec<String>),
    /// 4: these sessions oso did not start run in it.
    External(Vec<String>),
    /// 5: its branch is not known to be merged, and why.
    NotMerged(String),
}

impl Check {
    /// Why removing it is refused, for a person.
    pub fn why(&self) -> String {
        match self {
            Self::Unknown(why) | Self::NotLinked(why) => why.clone(),
            Self::Dirty(Status { changed, untracked }) => {
                format!("{changed} changed and {untracked} untracked files are not committed")
            }
            Self::Unsaved(1) => "a buffer in it has unsaved changes".to_owned(),
            Self::Unsaved(n) => format!("{n} buffers in it have unsaved changes"),
            Self::Unsent => "a brief for it is not sent".to_owned(),
            Self::Busy(names) => format!("session {} in it is busy", names.join(", ")),
            Self::External(names) => {
                format!(
                    "a session oso did not start runs in it: {}",
                    names.join(", ")
                )
            }
            Self::NotMerged(why) => format!("its branch is {why}"),
        }
    }
}

/// What is known of one linked worktree.
#[derive(Clone, Debug)]
pub struct Looked {
    pub listed: Listed,
    /// Check 1, from git's words alone: `Err` with why it cannot be removed.
    pub linked: Result<(), String>,
    /// Its changes, if they could be read.
    pub status: Option<Status>,
    /// Its branch's merge, once asked.
    pub merged: Option<Merged>,
    /// What git ignores in it, which a removal deletes: directories end with `/`.
    pub ignored: Vec<PathBuf>,
    /// Sessions oso did not start that run in it, by name or process id.
    pub external: Vec<String>,
    /// The hosted sessions in it that were busy when the job looked, by name: the registry's
    /// half of check 4, which only the job reads.
    pub busy: Vec<String>,
    /// The tip of its branch, when only a merged pull request can tell whether it is merged
    /// and a row's look left `gh` to be asked after every row.
    pub gh_pending: Option<String>,
}

/// How far [`look`] goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// Every fact but what git ignores, and no `gh`, for a row of `worktree-list`: `gh` is
    /// asked after every row, as [`Looked::gh_pending`] says.
    Local,
    /// In ADR 0121's order, stopping at the first check that fails, for a removal.
    UntilRefused,
}

/// Looks at the linked worktree `listed` of `repo`, with what the job read of the repository
/// once, asking `gh` if ancestry is not enough and `depth` allows.
pub fn look(
    repo: &Repo,
    gh: Option<&Gh>,
    facts: &RepoFacts,
    listed: Listed,
    held: &Held,
    depth: Depth,
) -> Looked {
    let mut looked = Looked {
        linked: linked(repo, &listed),
        listed,
        status: None,
        merged: None,
        ignored: Vec::new(),
        external: Vec::new(),
        busy: held.busy.clone(),
        gh_pending: None,
    };
    let git = repo.in_worktree(&looked.listed.path);
    let stop = |looked: &Looked| depth == Depth::UntilRefused && first(looked, held).is_some();
    if looked.linked.is_ok() && git.is_none() {
        looked.linked = Err("its .git does not name this repository back".to_owned());
    }
    // Check 1 first: what only the UI thread holds is checks 3 and 4, after check 2.
    if depth == Depth::UntilRefused && looked.linked.is_err() {
        return looked;
    }
    if let Some(git) = &git {
        let args = ["status", "--porcelain", "-z", "--untracked-files=all"];
        looked.status = run(git, &args, MAX_LISTING)
            .ok()
            .map(|out| porcelain::status(&out));
        // What a removal deletes is named only in its question.
        if depth == Depth::UntilRefused {
            looked.ignored = ignored(git);
        }
    }
    looked.external = facts.external_in(&looked.listed.path);
    if stop(&looked) {
        return looked;
    }
    let merged = match (&looked.listed.branch, &looked.listed.head) {
        (Some(branch), Some(_)) => match merged::tip(repo, branch) {
            Some(tip) if depth == Depth::Local => {
                match merged::locally(repo, facts, branch, &tip, gh.is_some()) {
                    Local::Done(merged) => merged,
                    Local::AskGh => {
                        looked.gh_pending = Some(tip);
                        Merged::No("not known yet: gh is being asked".to_owned())
                    }
                }
            }
            Some(tip) => merged::merged(repo, gh, facts, branch, &tip),
            None => Merged::No("not known: its tip cannot be read".to_owned()),
        },
        _ => Merged::No("not merged: no branch is checked out".to_owned()),
    };
    looked.merged = Some(merged);
    looked
}

/// What git ignores in the worktree `git` runs in, which a removal deletes.
fn ignored(git: &Git) -> Vec<PathBuf> {
    let args = [
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ];
    run(git, &args, MAX_LISTING)
        .map(|out| porcelain::paths(&out, MAX_IGNORED))
        .unwrap_or_default()
}

/// Check 1 from git's listing: a linked worktree, not the main one, not bare, locked or
/// prunable, whose directory is there.
fn linked(repo: &Repo, listed: &Listed) -> Result<(), String> {
    if listed.bare || oso_core::project::same_place(&listed.path, repo.main()) {
        return Err("it is the repository's main worktree".to_owned());
    }
    if listed.locked {
        return Err("it is locked (git worktree unlock)".to_owned());
    }
    if listed.prunable || !listed.path.is_dir() {
        return Err("its directory is gone (git worktree prune clears it)".to_owned());
    }
    Ok(())
}

/// The first check that does not hold, in ADR 0121's order, from what is known so far: a fact
/// not looked at yet does not refuse here, so a removal asks for every fact first.
pub fn first(looked: &Looked, held: &Held) -> Option<Check> {
    if let Err(why) = &looked.linked {
        return Some(Check::NotLinked(why.clone()));
    }
    if let Some(status) = looked.status
        && (status.changed > 0 || status.untracked > 0)
    {
        return Some(Check::Dirty(status));
    }
    if let Some(check) = held_refusal(held) {
        return Some(check);
    }
    if !looked.busy.is_empty() {
        return Some(Check::Busy(looked.busy.clone()));
    }
    if !looked.external.is_empty() {
        return Some(Check::External(looked.external.clone()));
    }
    match &looked.merged {
        Some(Merged::No(why)) => Some(Check::NotMerged(why.clone())),
        _ => None,
    }
}

/// The first of checks 3 and 4 that what the UI thread holds fails: unsaved buffers, an unsent
/// brief, busy sessions.
pub fn held_refusal(held: &Held) -> Option<Check> {
    if held.unsaved > 0 {
        return Some(Check::Unsaved(held.unsaved));
    }
    if held.unsent {
        return Some(Check::Unsent);
    }
    (!held.busy.is_empty()).then(|| Check::Busy(held.busy.clone()))
}

/// Whether every check holds: each fact was read, and none refuses.
pub fn removable(looked: &Looked, held: &Held) -> Result<(), Check> {
    if let Some(check) = first(looked, held) {
        return Err(check);
    }
    if looked.status.is_none() {
        return Err(Check::Unknown(
            "its changes cannot be read, so it is not known to be clean".to_owned(),
        ));
    }
    if !looked.merged.as_ref().is_some_and(Merged::is_merged) {
        return Err(Check::NotMerged("not known to be merged".to_owned()));
    }
    Ok(())
}

/// Removes the linked worktree at `path` with `git worktree remove`, never `--force`: git
/// itself refuses one that has changes by then. Its branch is kept.
///
/// # Errors
///
/// What git said.
pub fn remove(repo: &Repo, path: &Path) -> Result<(), String> {
    let Some(path) = path.to_str() else {
        return Err("its path is not UTF-8".to_owned());
    };
    run(repo.git(), &["worktree", "remove", "--", path], MAX_LISTING)
        .map(drop)
        .map_err(|e| said(&e))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{HostedSession, busy};

    fn session(settled: bool) -> HostedSession {
        HostedSession {
            directory: PathBuf::from("/r/.claude/worktrees/a"),
            name: "a".to_owned(),
            pid: Some(42),
            settled,
        }
    }

    #[test]
    fn a_working_session_is_busy_whatever_the_registry_says() {
        assert!(busy(&session(false), true));
        assert!(busy(&session(false), false));
    }

    #[test]
    fn an_idle_screen_with_a_busy_registry_is_busy() {
        assert!(busy(&session(true), false));
    }

    #[test]
    fn idle_on_screen_and_in_the_registry_is_not_busy() {
        assert!(!busy(&session(true), true));
    }
}
