//! Transport for the dev control socket (ADR 0016, ADR 0032). Dev builds
//! only: this module exists only with the `dev` feature.
//!
//! Must not depend on: `text` or the protocol spoken over the socket; it never
//! interprets the bytes it moves.
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
    use super::{is_plain_name, owner_pid, socket_pid};

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
