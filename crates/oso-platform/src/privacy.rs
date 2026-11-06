//! Keeping what the editor holds private to its user: no core dumps, and a directory
//! only the user can reach (ADR 0028, ADR 0062).
//!
//! Must not depend on: `control`, `process` or `text`. `control` uses the checks here, so
//! that the two kinds of private directory cannot drift apart.

// The checks serve the Unix backend, and are tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "used only by the Unix backend"))]

use std::io;
use std::path::{Component, Path, PathBuf};

#[cfg(not(unix))]
mod other;
#[cfg(not(unix))]
use other as backend;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as backend;

#[cfg(all(unix, feature = "dev"))]
pub(crate) use unix::euid;

/// Turns core dumps off for this process and whatever it starts, for good: a dump would
/// hold every buffer. Windows has no such limit for a process to set, so there this does
/// nothing and succeeds. A system-wide crash dump setting stays the user's.
///
/// # Errors
///
/// If the system refuses.
pub fn disable_core_dumps() -> io::Result<()> {
    backend::disable_core_dumps()
}

/// The per-user directory for state: `$XDG_STATE_HOME` or `~/.local/state`,
/// `~/Library/Application Support` on macOS, and `%LOCALAPPDATA%` on Windows. Only an absolute
/// value counts. Asks the environment and not the disk.
pub fn state_home() -> Option<PathBuf> {
    backend::state_home_from(&|name| std::env::var_os(name))
}

/// A directory only this user can reach, checked when it is opened and created if it
/// is missing. On Unix, that means mode `0700`, this user's own, not a symbolic link, and
/// nothing that leads to it can be changed by another user. Windows has no modes, so there it means a real
/// directory, and privacy comes from the per-user place the caller chose.
#[derive(Debug, Clone)]
pub struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    /// Opens the directory at `path`, creating it and what leads to it if missing, once
    /// [`Self::check`] has passed, so that a refused place gets nothing new. A directory
    /// found with a looser mode is refused, and not repaired, as something may already have
    /// read what is in it.
    ///
    /// # Errors
    ///
    /// If it cannot be created, or cannot be made safe. The message names the path and why.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = plain(&path.into())?;
        backend::open_private(&path)?;
        Ok(Self { path })
    }

    /// Whether [`Self::open`] would refuse the directory at `path`, creating nothing. It
    /// touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// Why it would be refused. The message names the path and why.
    pub fn check(path: &Path) -> io::Result<()> {
        backend::check_private(&plain(path)?)
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// `path`, spelled so that its last name is the directory itself: a final `/` or `/.` would
/// make the system follow a link there. A relative path would be judged by whatever the current
/// directory is, and `..` makes what is written differ from what would be created.
fn plain(path: &Path) -> io::Result<PathBuf> {
    let parent_dir = path.components().any(|c| c == Component::ParentDir);
    if path.is_absolute() && !parent_dir {
        Ok(path.components().collect())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not an absolute path without ..", path.display()),
        ))
    }
}

/// Why a directory cannot be private.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirProblem {
    Symlink,
    NotADirectory,
    WrongOwner,
    LooseMode,
}

impl DirProblem {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Symlink => "is a symlink",
            Self::NotADirectory => "is not a directory",
            Self::WrongOwner => "is owned by another user",
            Self::LooseMode => "is accessible by other users (mode is not 0700)",
        }
    }
}

/// Checks a directory's `lstat` results: it must be a real directory, owned
/// by `euid`, with no permissions for group or others.
pub(crate) fn check_dir(
    is_symlink: bool,
    is_dir: bool,
    owner: u32,
    mode: u32,
    euid: u32,
) -> Result<(), DirProblem> {
    if is_symlink {
        Err(DirProblem::Symlink)
    } else if !is_dir {
        Err(DirProblem::NotADirectory)
    } else if owner != euid {
        Err(DirProblem::WrongOwner)
    } else if mode & 0o077 != 0 {
        Err(DirProblem::LooseMode)
    } else {
        Ok(())
    }
}

/// Whether a parent directory with `mode` keeps other users from replacing
/// what is in it: not writable by group or others, or sticky.
pub(crate) fn parent_is_safe(mode: u32) -> bool {
    mode & 0o022 == 0 || mode & 0o1000 != 0
}

/// Whether a directory leading to a private one keeps other users out: safe as a parent, and
/// this user's or root's. Root owns what leads to many home directories, and can replace
/// anything anyway.
pub(crate) fn ancestor_is_safe(mode: u32, owner: u32, euid: u32) -> bool {
    parent_is_safe(mode) && (owner == euid || owner == 0)
}

#[cfg(test)]
mod tests {
    use super::{DirProblem, ancestor_is_safe, check_dir, parent_is_safe};

    #[test]
    fn parents_must_be_private_or_sticky() {
        assert!(parent_is_safe(0o700));
        assert!(parent_is_safe(0o755));
        assert!(parent_is_safe(0o1777));
        assert!(!parent_is_safe(0o777));
        assert!(!parent_is_safe(0o775));
        assert!(!parent_is_safe(0o757));
    }

    #[test]
    fn what_leads_to_a_private_directory_must_be_this_users_or_roots() {
        assert!(ancestor_is_safe(0o755, 501, 501));
        assert!(ancestor_is_safe(0o755, 0, 501));
        assert!(ancestor_is_safe(0o1777, 0, 501));
        assert!(!ancestor_is_safe(0o755, 502, 501), "another user's");
        assert!(!ancestor_is_safe(0o775, 501, 501), "others can write");
    }

    #[test]
    fn accepts_a_private_directory() {
        assert_eq!(check_dir(false, true, 501, 0o700, 501), Ok(()));
        assert_eq!(check_dir(false, true, 501, 0o500, 501), Ok(()));
    }

    #[test]
    fn refuses_a_symlink() {
        assert_eq!(
            check_dir(true, false, 501, 0o777, 501),
            Err(DirProblem::Symlink)
        );
    }

    #[test]
    fn refuses_a_file() {
        assert_eq!(
            check_dir(false, false, 501, 0o600, 501),
            Err(DirProblem::NotADirectory)
        );
    }

    #[test]
    fn refuses_another_owner() {
        assert_eq!(
            check_dir(false, true, 0, 0o700, 501),
            Err(DirProblem::WrongOwner)
        );
        assert_eq!(
            check_dir(false, true, 502, 0o700, 501),
            Err(DirProblem::WrongOwner)
        );
    }

    #[test]
    fn refuses_any_group_or_other_permission() {
        for mode in [0o755, 0o750, 0o701, 0o710, 0o707, 0o1777] {
            assert_eq!(
                check_dir(false, true, 501, mode, 501),
                Err(DirProblem::LooseMode),
                "{mode:o}"
            );
        }
    }

    #[test]
    fn state_home_comes_from_the_environment_and_only_an_absolute_path_counts() {
        use super::backend::state_home_from;
        use std::path::PathBuf;
        let home = |vars: &'static [(&'static str, &'static str)]| {
            state_home_from(&|name| {
                vars.iter()
                    .find(|(var, _)| *var == name)
                    .map(|(_, value)| value.into())
            })
        };
        assert_eq!(home(&[]), None);
        assert_eq!(
            home(&[("HOME", "relative"), ("LOCALAPPDATA", "relative")]),
            None
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            home(&[("HOME", "/h"), ("XDG_STATE_HOME", "/x")]),
            Some(PathBuf::from("/h/Library/Application Support"))
        );
        #[cfg(windows)]
        assert_eq!(
            home(&[("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local")]),
            Some(PathBuf::from("C:\\Users\\a\\AppData\\Local"))
        );
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            assert_eq!(
                home(&[("HOME", "/h"), ("XDG_STATE_HOME", "/x")]),
                Some(PathBuf::from("/x"))
            );
            assert_eq!(
                home(&[("HOME", "/h"), ("XDG_STATE_HOME", "relative")]),
                Some(PathBuf::from("/h/.local/state"))
            );
        }
    }
}
