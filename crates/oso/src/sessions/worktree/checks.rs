//! What is known of one linked worktree, and the five checks `worktree-remove` runs before it
//! removes one, in ADR 0121's order: a linked worktree of this repository; nothing uncommitted
//! or untracked; no unsaved buffer or unsent brief; no busy session; its branch merged. A
//! worktree whose removal runs is refused before them all (`Check::Removing`).
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; [`look`] runs on a
//! worktree job's thread, and what only the UI thread knows (buffers, hosted sessions) comes
//! with the job as [`Held`].
//!
//! A check that cannot be run refuses: a worktree is removed only when every check holds.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use super::facts::RepoFacts;
use super::git::{MAX_LISTING, Repo, run, said};
use super::merged::{self, Local, Merged};
use super::porcelain::{self, Status};
use crate::git::ListedWorktree;
use crate::git::{Gh, Git, GitError};

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
    /// The state its screen gives is idle, done or ready for review. A session ready for
    /// review in a worktree checks 2 and 5 let go has its work committed and merged.
    pub settled: bool,
}

/// Whether a hosted session that runs is busy, for check 4 (the author's choice, ADR 0121):
/// unless its screen's state is idle, done or ready for review *and* Claude Code's registry
/// says `idle` for its process (`registry_idle`). Each half alone can miss a turn or a
/// background task. Ready for review is settled because checks 2 and 5 refuse its work unless
/// it is committed and merged ("Merged work is not ready for review, as amended").
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
    /// A removal of it runs already.
    Removing,
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
            Self::Removing => "a removal of it still runs".to_owned(),
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
    pub listed: ListedWorktree,
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
    /// Why Claude Code's registry could not be listed in full, if it could not: check 4 cannot
    /// tell whether a session oso did not start runs in it.
    pub registry_unread: Option<&'static str>,
    /// The hosted sessions in it that were busy when the job looked, by name: the registry's
    /// half of check 4, which only the job reads.
    pub busy: Vec<String>,
    /// The tip of its branch, when only a merged pull request can tell whether it is merged
    /// and a row's look left `gh` to be asked after every row.
    pub gh_pending: Option<String>,
    /// A removal of it ran when the job looked.
    pub removing: bool,
    /// When the job began to read its changes.
    pub looked_at: Instant,
}

impl Looked {
    /// Whether its changes were read and none is uncommitted or untracked: check 2 holds.
    pub fn clean(&self) -> bool {
        self.status
            .is_some_and(|status| status.changed == 0 && status.untracked == 0)
    }

    /// Whether it is clean and its branch is known to be merged: what a session in it changed
    /// needs no review (ADR 0121, "Merged work is not ready for review, as amended").
    pub fn work_merged(&self) -> bool {
        self.clean() && self.merged.as_ref().is_some_and(Merged::is_merged)
    }
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
    listed: ListedWorktree,
    held: &Held,
    depth: Depth,
) -> Looked {
    let mut looked = Looked {
        looked_at: Instant::now(),
        linked: linked(repo, &listed),
        listed,
        status: None,
        merged: None,
        ignored: Vec::new(),
        external: Vec::new(),
        registry_unread: None,
        busy: held.busy.clone(),
        gh_pending: None,
        removing: false,
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
    looked.registry_unread = facts.registry_unread;
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
fn linked(repo: &Repo, listed: &ListedWorktree) -> Result<(), String> {
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
    if looked.removing {
        return Some(Check::Removing);
    }
    if let Err(why) = &looked.linked {
        return Some(Check::NotLinked(why.clone()));
    }
    if let Some(status) = looked.status
        && (status.changed > 0 || status.untracked > 0)
    {
        return Some(Check::Dirty(status));
    }
    // 4 cannot be run on a registry not listed in full. Its refusal comes before a busy
    // session's, as an entry not read leaves a settled session busy too.
    if let Some(why) = looked.registry_unread
        && held.unsaved == 0
        && !held.unsent
    {
        return Some(Check::Unknown(why.to_owned()));
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
/// git runs with no time limit, in a session of its own with no controlling terminal, so a
/// filter that opens the terminal fails instead of stopping. Once it runs, every check has held
/// and it may be deleting, and a kill would leave the worktree half removed, which check 2 then
/// refuses: only `stop`, set by the person's `worktree-remove-stop` after a warning, stops it.
/// Deleting a large ignored `target/` can take longer than the 10 s of every other run (ADR
/// 0121, "Worktree removal, as fixed"). Quitting oso leaves it to finish.
///
/// # Errors
///
/// What git said.
pub fn remove(repo: &Repo, path: &Path, stop: &AtomicBool) -> Result<(), String> {
    let Some(path) = path.to_str() else {
        return Err("its path is not UTF-8".to_owned());
    };
    let args = ["worktree", "remove", "--", path].map(OsStr::new);
    let git = repo.git().detached();
    let out = (git.run_long(&args, &[], None, stop)).map_err(|e| said(&e))?;
    if out.status.success() {
        return Ok(());
    }
    Err(said(&GitError::Failed {
        status: out.status.code(),
        stderr: out.stderr,
    }))
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

    /// A clean linked worktree, its merge not asked yet, with nothing else in it.
    fn looked() -> super::Looked {
        use super::super::porcelain::Status;
        use crate::git::ListedWorktree;
        super::Looked {
            listed: ListedWorktree {
                path: PathBuf::from("/r/.claude/worktrees/a"),
                head: None,
                branch: Some("a".to_owned()),
                bare: false,
                detached: false,
                locked: false,
                prunable: false,
            },
            linked: Ok(()),
            status: Some(Status::default()),
            merged: None,
            ignored: Vec::new(),
            external: Vec::new(),
            registry_unread: None,
            busy: Vec::new(),
            gh_pending: None,
            removing: false,
            looked_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn a_clean_merged_worktree_is_its_sessions_work_merged() {
        use super::super::merged::Merged;
        use super::super::porcelain::Status;
        let mut looked = looked();
        looked.merged = Some(Merged::Ancestor("origin/main".to_owned()));
        assert!(looked.work_merged());
        looked.status = Some(Status {
            changed: 0,
            untracked: 1,
        });
        assert!(
            !looked.work_merged(),
            "an untracked file is work not merged"
        );
        looked.status = None;
        assert!(!looked.work_merged(), "changes not read");
        looked.status = Some(Status::default());
        looked.merged = Some(Merged::No("not merged: never pushed".to_owned()));
        assert!(!looked.work_merged());
        looked.merged = None;
        assert!(!looked.work_merged(), "not asked yet");
    }

    #[test]
    fn a_registry_not_listed_in_full_refuses_after_check_3_and_before_busy() {
        use super::super::merged::Merged;
        use super::{Check, Held, removable};
        let mut looked = looked();
        looked.merged = Some(Merged::Ancestor("origin/main".to_owned()));
        assert_eq!(removable(&looked, &Held::default()), Ok(()));
        looked.registry_unread = Some("not listed");
        let unknown = Err(Check::Unknown("not listed".to_owned()));
        assert_eq!(removable(&looked, &Held::default()), unknown);
        let busy = Held {
            busy: vec!["a".to_owned()],
            ..Held::default()
        };
        assert_eq!(removable(&looked, &busy), unknown);
        looked.busy = vec!["a".to_owned()];
        looked.external = vec!["b".to_owned()];
        assert_eq!(removable(&looked, &Held::default()), unknown);
        let unsaved = Held {
            unsaved: 1,
            ..Held::default()
        };
        assert_eq!(removable(&looked, &unsaved), Err(Check::Unsaved(1)));
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
