//! The index's lock, `index.lock` (ADR 0168): where git keeps
//! it, what is there before and after a run, whether a stop left one or a failure names one,
//! and taking a stale one away once the person said `y`.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these look at the disk and run
//! git, on the repository's run thread and the status's.
//!
//! - Where the lock is comes from git (`rev-parse --git-path index`), never from a guess: a
//!   worktree's is in its own git directory.
//! - A file there is known by its device and inode, looked at with no link followed; on
//!   Windows, where the standard library gives neither, by its creation time, its last write
//!   time and its length, a weaker stand-in.
//! - Nothing here takes a lock away unasked: a git the person started elsewhere may own a
//!   newer one. [`Lock::remove`] takes away only the file noted, still the same.

use std::fs::Metadata;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::git::Git;

/// What the echo area adds when a stop left the lock.
pub(crate) const STOP_LEFT: &str =
    "git writes no index while it is there; M-x git-remove-stale-lock removes it";

/// Where git keeps the index's lock, and how the echo area names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lock {
    path: PathBuf,
    shown: String,
}

/// Which file was at the lock's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Id {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    /// When its inode last changed: a number freed and given to a new file, as ext4 does,
    /// comes with a new time.
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(not(unix))]
    created: Option<SystemTime>,
    #[cfg(not(unix))]
    modified: Option<SystemTime>,
    #[cfg(not(unix))]
    len: u64,
}

/// A file at the lock's path, as it was looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) id: Id,
    /// When it was made, or last written where the system keeps no creation time.
    pub(crate) made: Option<SystemTime>,
}

/// A lock noted after a run: a stop left it, or git failed on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Noted {
    pub(crate) lock: Lock,
    pub(crate) seen: Seen,
    /// Whether a stop left it; else a run failed on it.
    pub(crate) stopped: bool,
}

impl Lock {
    /// Where `git` keeps the index's lock; `None` if git cannot say.
    pub(crate) fn of(git: &Git) -> Option<Lock> {
        let index = super::read::git_path(git, "index")?;
        Some(Lock::at(&index, git.top()))
    }

    /// The lock of the index at `index`, named from `top` when it is under it.
    pub(crate) fn at(index: &Path, top: &Path) -> Lock {
        let mut path = index.as_os_str().to_owned();
        path.push(".lock");
        let path = PathBuf::from(path);
        let named = path.strip_prefix(top).unwrap_or(&path);
        let shown = oso_core::editor::row_text(&named.to_string_lossy());
        Lock { path, shown }
    }

    /// How the echo area names it: from the work tree's top when it is under it.
    pub(crate) fn shown(&self) -> &str {
        &self.shown
    }

    /// What is at its path now, no link followed; `None` if nothing, or if it cannot be looked
    /// at.
    pub(crate) fn look(&self) -> Option<Seen> {
        let meta = self.path.symlink_metadata().ok()?;
        Some(Seen {
            id: Id::of(&meta),
            made: meta.created().or_else(|_| meta.modified()).ok(),
        })
    }

    /// What a run that ended, with `before` at the path as it began, leaves noted: a stop
    /// (`stopped`) left a lock that was not there, or another file; a run that failed with
    /// output naming the lock (`named`) found the one there as it began, still there. `None`
    /// otherwise.
    pub(crate) fn after(&self, before: Option<Seen>, stopped: bool, named: bool) -> Option<Noted> {
        if !stopped && !named {
            return None;
        }
        let seen = self.look()?;
        let left = before.is_none_or(|before| before.id != seen.id);
        // A stop notes a lock it may have left; a failure only one there as the run began,
        // which git then found, and not one another git made meanwhile.
        if left != stopped {
            return None;
        }
        Some(Noted {
            lock: self.clone(),
            seen,
            stopped,
        })
    }

    /// Takes the lock away if it is still the file `id`: what the echo area says, and whether
    /// the note stays, as the same file is still there.
    pub(crate) fn remove(&self, id: Id) -> (String, bool) {
        let shown = &self.shown;
        let gone = || (format!("No {shown} now: nothing to remove"), false);
        let Some(now) = self.look() else {
            return gone();
        };
        if now.id != id {
            let said =
                format!("{shown} is another file now: another git may own it, so oso leaves it");
            return (said, false);
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => (format!("Removed {shown}"), false),
            Err(e) if e.kind() == ErrorKind::NotFound => gone(),
            Err(e) => (format!("Cannot remove {shown}: {e}"), true),
        }
    }
}

impl Id {
    #[cfg(unix)]
    fn of(meta: &Metadata) -> Id {
        use std::os::unix::fs::MetadataExt;
        Id {
            dev: meta.dev(),
            ino: meta.ino(),
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }

    #[cfg(not(unix))]
    fn of(meta: &Metadata) -> Id {
        Id {
            created: meta.created().ok(),
            modified: meta.modified().ok(),
            len: meta.len(),
        }
    }
}

/// Whether what a run wrote names the index's lock as git's refusal does, `Unable to create
/// '…/index.lock': File exists.`, in whatever language git speaks, as the quoted path is not
/// translated; what a remote said (`remote:`) is not looked at.
pub(crate) fn named(output: &str) -> bool {
    (output.lines())
        .filter(|line| !line.trim_start().starts_with("remote:"))
        .any(|line| line.contains("index.lock'"))
}

/// What the echo area says of a run that failed on `lock`.
pub(crate) fn found(lock: &Lock) -> String {
    format!(
        "git found {}: another git may run, or a stop left it; M-x git-remove-stale-lock \
         removes it",
        lock.shown
    )
}

/// What the echo area says of a stop that left `lock`.
pub(crate) fn left(lock: &Lock) -> String {
    format!("the stop left {}: {STOP_LEFT}", lock.shown)
}

#[cfg(test)]
mod tests;
