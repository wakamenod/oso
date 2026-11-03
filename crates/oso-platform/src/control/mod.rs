//! Transport for the dev control socket (ADR 0016, ADR 0031). Dev builds
//! only: this module exists only with the `dev` feature.
//!
//! - The socket lives in a private directory: `$XDG_RUNTIME_DIR/oso` when
//!   that is set, otherwise `oso-<uid>` in the temporary directory (which is
//!   per user on macOS). It is created with mode `0700`, and refused if it is
//!   a symlink, is owned by another user or has a looser mode.
//! - Each instance listens on `<pid>.sock` in that directory. Files it
//!   writes there are named `<pid>-<name>`, mode `0600`.
//! - Both ends check that the peer runs as the same user.
//! - Sockets that refuse connections are stale; binding removes them along
//!   with their files.
//!
//! Only Unix is supported for now. On other systems every operation fails
//! with [`std::io::ErrorKind::Unsupported`].

// The helpers below serve the Unix transport and are tested everywhere.
#![cfg_attr(
    not(unix),
    allow(dead_code, reason = "used only by the Unix transport")
)]

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{CleanupGuard, ControlDir, ControlListener, ControlStream};

#[cfg(not(unix))]
mod unsupported;
#[cfg(not(unix))]
pub use unsupported::{CleanupGuard, ControlDir, ControlListener, ControlStream};

/// Why a directory cannot hold the control socket.
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

/// The process ID in a socket file name, `<pid>.sock`.
pub(crate) fn socket_pid(name: &str) -> Option<u32> {
    parse_pid(name.strip_suffix(".sock")?)
}

/// The process ID that owns a file, `<pid>-<name>` or `<pid>.sock`.
pub(crate) fn owner_pid(name: &str) -> Option<u32> {
    socket_pid(name).or_else(|| parse_pid(name.split_once('-')?.0))
}

fn parse_pid(digits: &str) -> Option<u32> {
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether `name` can be written with [`ControlDir::write_private_file`]:
/// a plain file name, not a path.
pub(crate) fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::{DirProblem, check_dir, is_plain_name, owner_pid, parent_is_safe, socket_pid};

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
    fn parses_file_names() {
        assert_eq!(socket_pid("123.sock"), Some(123));
        assert_eq!(socket_pid("0123.sock"), None);
        assert_eq!(socket_pid("abc.sock"), None);
        assert_eq!(socket_pid(".sock"), None);
        assert_eq!(socket_pid("123"), None);
        assert_eq!(owner_pid("123.sock"), Some(123));
        assert_eq!(owner_pid("123-screenshot-1.png"), Some(123));
        assert_eq!(owner_pid("x-1.png"), None);
        assert_eq!(owner_pid("99999999999-a"), None);
    }

    #[test]
    fn plain_names_only() {
        assert!(is_plain_name("screenshot-1.png"));
        for bad in ["", ".", "..", "a/b", "../a", "a\\b", ".hidden", "a\0b"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }
}
