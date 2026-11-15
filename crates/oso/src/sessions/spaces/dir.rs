//! A space's own directory (ADR 0136, Q1): `<root>/<name>`, where the orchestrator starts,
//! outside every repository. oso makes it private and checks it before each use; it reads
//! nothing in it.
//!
//! Must not depend on: threads, the UI or `app`; it touches the disk, so it runs on a worker.
//!
//! - The root, and each directory on the way to it, must be writable by no one but the person
//!   and root, as `config.toml`'s directories are (ADR 0108). A shared one is refused, never
//!   repaired: another user could have put hooks or a `CLAUDE.md` there.
//! - The space's directory is mode `0700`, the person's own, not a link, as
//!   [`PrivateDir`] has it, with the device and inode it had when it was made: one moved away,
//!   or another put in its place, is refused, as Claude Code keys its memory by the path.
//! - Neither holds `.git`, and neither is inside one of the space's repositories.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "space-new makes and checks directories in step 1b"
    )
)]

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use oso_platform::privacy::{PrivateDir, Writers, who_can_change};

use super::stored::Member;

/// Why a space's directory may not be used.
#[derive(Debug)]
pub enum Refusal {
    /// The root, or a directory on the way to it, that others can write.
    Shared(PathBuf),
    /// The directory is missing at its path.
    Missing(PathBuf),
    /// Another directory is at its path than the one oso made.
    Replaced(PathBuf),
    /// It, or the root, is a repository.
    Repository(PathBuf),
    /// It is inside one of the space's repositories.
    InsideRepository(PathBuf),
    /// It is not private, or could not be looked at: the system's words.
    Refused(PathBuf, io::Error),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared(path) => write!(
                f,
                "space directory refused: others can change {}; set [spaces] root to a \
                 directory only you can write",
                path.display()
            ),
            Self::Missing(path) => write!(f, "space directory missing: {}", path.display()),
            Self::Replaced(path) => write!(
                f,
                "space directory replaced: {} is not the directory oso made",
                path.display()
            ),
            Self::Repository(path) => {
                write!(
                    f,
                    "space directory refused: {} is a repository",
                    path.display()
                )
            }
            Self::InsideRepository(path) => write!(
                f,
                "space directory refused: {} is inside a repository of the space",
                path.display()
            ),
            Self::Refused(path, e) => {
                write!(f, "space directory refused: {}: {e}", path.display())
            }
        }
    }
}

/// Where the directory of a space called `name` goes under `root`.
pub fn place(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

/// Makes the directory of a space called `name` under `root`, and `root` if missing, both
/// private, for a space of `members`. Gives its path and identity.
///
/// # Errors
///
/// A [`Refusal`]; nothing is made under a root that is refused.
pub fn make(
    root: &Path,
    name: &str,
    members: &[Member],
) -> Result<(PathBuf, Option<(u64, u64)>), Refusal> {
    let path = place(root, name);
    usable(root, &path, members)?;
    if root.exists() {
        unshared(root)?;
    }
    let made = PrivateDir::open(&path).map_err(|e| Refusal::Refused(path.clone(), e))?;
    let meta = fs::symlink_metadata(made.path()).map_err(|e| Refusal::Refused(path.clone(), e))?;
    usable(root, &path, members)?;
    Ok((path, identity(&meta)))
}

/// Checks the directory at `path`, made with `made` as its identity, for a space of
/// `members`, before a session starts there.
///
/// # Errors
///
/// A [`Refusal`].
pub fn check(path: &Path, made: Option<(u64, u64)>, members: &[Member]) -> Result<(), Refusal> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Refusal::Missing(path.to_owned()));
        }
        Err(e) => return Err(Refusal::Refused(path.to_owned(), e)),
    };
    if !meta.is_dir() || (made.is_some() && identity(&meta) != made) {
        return Err(Refusal::Replaced(path.to_owned()));
    }
    let root = path.parent().unwrap_or(path);
    unshared(root)?;
    match PrivateDir::existing(path) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(Refusal::Missing(path.to_owned())),
        Err(e) => return Err(Refusal::Refused(path.to_owned(), e)),
    }
    usable(root, path, members)
}

/// Refuses a `root` that someone else can change, or the way to it.
fn unshared(root: &Path) -> Result<(), Refusal> {
    let who = who_can_change(root).map_err(|e| Refusal::Refused(root.to_owned(), e))?;
    if who.file == Writers::Nobody && who.way == Writers::Nobody {
        Ok(())
    } else {
        Err(Refusal::Shared(root.to_owned()))
    }
}

/// Refuses a root or a directory that is a repository or inside one of `members`.
fn usable(root: &Path, path: &Path, members: &[Member]) -> Result<(), Refusal> {
    for dir in [root, path] {
        if fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Err(Refusal::Repository(dir.to_owned()));
        }
    }
    if members.iter().any(|member| path.starts_with(&member.root)) {
        return Err(Refusal::InsideRepository(path.to_owned()));
    }
    Ok(())
}

/// A directory's device and inode, where the system has them.
pub fn identity(meta: &fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    // `std` keeps a file's identity unstable here, so the path alone counts (ADR 0114).
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

#[cfg(all(test, unix))]
mod tests;
