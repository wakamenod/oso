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

use oso_platform::process;

use super::git::{Gh, MAX_LISTING, Repo, run, said};
use super::merged::{self, Merged};
use super::porcelain::{self, Listed, Status};
use crate::sessions::registry;

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
    /// The hosted sessions in it that are working, need input or may have a background task,
    /// by name.
    pub busy: Vec<String>,
    /// The process ids of `claude`s oso hosts, which the registry lists too.
    pub hosted: Vec<u32>,
}

/// A check that does not hold, by its number in ADR 0121's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
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
            Self::NotLinked(why) => why.clone(),
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
}

/// How far [`look`] goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// Every fact, for a row of `worktree-list`.
    All,
    /// In ADR 0121's order, stopping at the first check that fails, for a removal.
    UntilRefused,
}

/// Looks at the linked worktree `listed` of `repo`, asking `gh` if ancestry is not enough.
pub fn look(repo: &Repo, gh: Option<&Gh>, listed: Listed, held: &Held, depth: Depth) -> Looked {
    let mut looked = Looked {
        linked: linked(repo, &listed),
        listed,
        status: None,
        merged: None,
        ignored: Vec::new(),
        external: Vec::new(),
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
        let args = [
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ];
        if let Ok(out) = run(git, &args, MAX_LISTING) {
            looked.ignored = porcelain::paths(&out, MAX_IGNORED);
        }
    }
    looked.external = external(&looked.listed.path, &held.hosted);
    if stop(&looked) {
        return looked;
    }
    looked.merged = Some(match (&looked.listed.branch, &looked.listed.head) {
        (Some(branch), Some(_)) => match merged::tip(repo, branch) {
            Some(tip) => merged::merged(repo, gh, branch, &tip),
            None => Merged::No("not known: its tip cannot be read".to_owned()),
        },
        _ => Merged::No("not merged: no branch is checked out".to_owned()),
    });
    looked
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

/// The live sessions in the registry whose directory is in `path`, but those oso hosts.
fn external(path: &Path, hosted: &[u32]) -> Vec<String> {
    let Some(dir) = registry::directory() else {
        return Vec::new();
    };
    registry::read(&dir)
        .into_iter()
        .filter(|entry| !hosted.contains(&entry.pid))
        .filter(|entry| {
            entry
                .cwd
                .as_deref()
                .is_some_and(|cwd| cwd.starts_with(path))
        })
        .filter(|entry| registry::liveness(entry, process::start_time) != registry::Live::No)
        .map(|entry| {
            entry.name.clone().map_or_else(
                || format!("process {}", entry.pid),
                |name| porcelain::printable(&name, 48),
            )
        })
        .collect()
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
    if held.unsaved > 0 {
        return Some(Check::Unsaved(held.unsaved));
    }
    if held.unsent {
        return Some(Check::Unsent);
    }
    if !held.busy.is_empty() {
        return Some(Check::Busy(held.busy.clone()));
    }
    if !looked.external.is_empty() {
        return Some(Check::External(looked.external.clone()));
    }
    match &looked.merged {
        Some(Merged::No(why)) => Some(Check::NotMerged(why.clone())),
        _ => None,
    }
}

/// Whether every check holds: each fact was read, and none refuses.
pub fn removable(looked: &Looked, held: &Held) -> Result<(), Check> {
    if let Some(check) = first(looked, held) {
        return Err(check);
    }
    if looked.status.is_none() {
        return Err(Check::NotLinked(
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
