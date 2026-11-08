//! Transport for the control sockets: the dev control socket (ADR 0016,
//! ADR 0032) and the agent channel (ADR 0085). It is in every build, but only
//! the dev tooling and the agent channel listen, and the agent channel only
//! once the person turns it on.
//!
//! Must not depend on: `text` or the protocol spoken over the socket; it never
//! interprets the bytes it moves.
//!
//! - The sockets live in a private directory: `$XDG_RUNTIME_DIR/oso` when
//!   that is set, otherwise `oso-<uid>` in the temporary directory (which is
//!   per user on macOS). It is created with mode `0700`, and refused if it is
//!   a symlink, is owned by another user or has a looser mode.
//! - Each instance listens on `<pid>.sock` (dev) or `<pid>-agent.sock` (the
//!   agent channel) in that directory. Files it writes there are named
//!   `<pid>-<name>`, mode `0600`.
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

/// Which of an instance's sockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socket {
    /// The dev control socket, `<pid>.sock` (ADR 0032).
    Dev,
    /// The agent channel, `<pid>-agent.sock` (ADR 0085).
    Agent,
}

impl Socket {
    /// The socket's file name for the instance with process ID `pid`.
    pub fn file_name(self, pid: u32) -> String {
        match self {
            Self::Dev => format!("{pid}.sock"),
            Self::Agent => format!("{pid}-agent.sock"),
        }
    }

    /// What it is called in a message.
    const fn what(self) -> &'static str {
        match self {
            Self::Dev => "oso dev instance",
            Self::Agent => "oso instance with the agent channel on",
        }
    }
}

/// The process at the other end of a connection, as the system reports it, for the agent
/// channel to name it to the person (ADR 0085). Each part is `None` where the system says
/// nothing. A process can end and its ID be reused, so this names, and proves nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Peer {
    /// The process that connected.
    pub process: Option<Process>,
    /// The process that started it.
    pub parent: Option<Process>,
}

/// A process, by ID and by the name the system gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// At most [`MAX_PROCESS_NAME`] bytes, without control characters.
    pub name: Option<String>,
}

/// The longest process name a [`Process`] holds, in bytes.
pub const MAX_PROCESS_NAME: usize = 64;

/// A process name as the system gave it, without control characters and cut to
/// [`MAX_PROCESS_NAME`] bytes, or `None` if nothing is left.
pub(crate) fn process_name(raw: &str) -> Option<String> {
    let mut name = String::new();
    for c in raw.chars().filter(|c| !c.is_control()) {
        if name.len() + c.len_utf8() > MAX_PROCESS_NAME {
            break;
        }
        name.push(c);
    }
    (!name.is_empty()).then_some(name)
}

/// The process ID and the socket in a socket file name.
pub(crate) fn socket_of(name: &str) -> Option<(u32, Socket)> {
    let stem = name.strip_suffix(".sock")?;
    match stem.strip_suffix("-agent") {
        Some(pid) => Some((parse_pid(pid)?, Socket::Agent)),
        None => Some((parse_pid(stem)?, Socket::Dev)),
    }
}

/// The process ID that owns a file, `<pid>-<name>` or `<pid>.sock`.
pub(crate) fn owner_pid(name: &str) -> Option<u32> {
    socket_of(name)
        .map(|(pid, _)| pid)
        .or_else(|| parse_pid(name.split_once('-')?.0))
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
    use super::{Socket, is_plain_name, owner_pid, process_name, socket_of};

    #[test]
    fn parses_file_names() {
        assert_eq!(socket_of("123.sock"), Some((123, Socket::Dev)));
        assert_eq!(socket_of("123-agent.sock"), Some((123, Socket::Agent)));
        assert_eq!(socket_of("0123.sock"), None);
        assert_eq!(socket_of("abc.sock"), None);
        assert_eq!(socket_of("abc-agent.sock"), None);
        assert_eq!(socket_of("-agent.sock"), None);
        assert_eq!(socket_of("123-other.sock"), None);
        assert_eq!(socket_of(".sock"), None);
        assert_eq!(socket_of("123"), None);
        assert_eq!(owner_pid("123.sock"), Some(123));
        assert_eq!(owner_pid("123-agent.sock"), Some(123));
        assert_eq!(owner_pid("123-screenshot-1.png"), Some(123));
        assert_eq!(owner_pid("x-1.png"), None);
        assert_eq!(owner_pid("99999999999-a"), None);
    }

    #[test]
    fn socket_file_names_parse_back() {
        for socket in [Socket::Dev, Socket::Agent] {
            assert_eq!(socket_of(&socket.file_name(42)), Some((42, socket)));
        }
    }

    #[test]
    fn process_names_lose_control_characters_and_length() {
        assert_eq!(process_name("claude"), Some("claude".to_owned()));
        assert_eq!(process_name("a\u{1b}[31mb\n"), Some("a[31mb".to_owned()));
        assert_eq!(process_name("\n\t"), None);
        let long = "é".repeat(40);
        let name = process_name(&long).unwrap();
        assert_eq!(name.len(), 64);
        assert!(long.starts_with(&name));
    }

    #[test]
    fn plain_names_only() {
        assert!(is_plain_name("screenshot-1.png"));
        for bad in ["", ".", "..", "a/b", "../a", "a\\b", ".hidden", "a\0b"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }
}
