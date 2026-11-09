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
    pub(crate) fn file_name(self, pid: u32) -> String {
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
/// channel to name it to the person (ADR 0085). What the system does not say is `None`. A
/// process can end and its ID be reused, and a process names itself, so this names and proves
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Peer {
    /// The process the system names for the other end. On Linux it is the one that connected.
    /// On macOS it is the last one that sent or received on the client's socket, which a
    /// process can hand on to another, and the system keeps it once that process has ended.
    pub process: Option<PeerProcess>,
    /// The process that started it.
    pub parent: Option<PeerProcess>,
    /// Whether the process that connected runs this program, by the path of its executable.
    /// A name can be anything, so only this tells `oso mcp` from a process called `oso`.
    pub runs_this_program: bool,
}

/// A process at the other end of a connection, by ID and by the name the system gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerProcess {
    pid: u32,
    name: Option<String>,
}

impl PeerProcess {
    /// A process with the name `raw`, cleaned as [`Self::name`] says.
    pub fn new(pid: u32, raw: Option<&str>) -> Self {
        Self {
            pid,
            name: raw.and_then(Self::clean_name),
        }
    }

    /// Its process ID.
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Its name, safe to show in a question: printable ASCII and spaces only, each other
    /// character as `\u{..}`, so that no control, bidi or invisible character in a name a
    /// process chose for itself can change how the question reads. At most
    /// [`MAX_PROCESS_NAME`] bytes, and `None` if the system gave none.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// `raw` as [`Self::name`] gives it, or `None` if nothing is left.
    pub(crate) fn clean_name(raw: &str) -> Option<String> {
        let mut name = String::new();
        for c in raw.chars() {
            // Quotes and parentheses would let a name close the quote the question puts
            // around it and speak for the question.
            let plain = c == ' ' || (c.is_ascii_graphic() && !matches!(c, '"' | '(' | ')'));
            let piece = if plain {
                c.to_string()
            } else {
                c.escape_unicode().to_string()
            };
            if name.len() + piece.len() > MAX_PROCESS_NAME {
                break;
            }
            name.push_str(&piece);
        }
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_owned())
    }
}

/// The longest process name a [`PeerProcess`] holds, in bytes.
pub(crate) const MAX_PROCESS_NAME: usize = 64;

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
    use super::{PeerProcess, Socket, is_plain_name, owner_pid, socket_of};

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
    fn process_names_keep_printable_ascii_and_escape_the_rest() {
        let name = |raw: &str| PeerProcess::clean_name(raw);
        assert_eq!(name("claude"), Some("claude".to_owned()));
        assert_eq!(name(" oso mcp "), Some("oso mcp".to_owned()));
        assert_eq!(name("a\u{1b}[31m"), Some("a\\u{1b}[31m".to_owned()));
        for invisible in [
            '\u{202e}', '\u{2066}', '\u{200b}', '\u{2028}', '\u{feff}', '\n',
        ] {
            let shown = name(&format!("x{invisible}y")).unwrap();
            assert!(shown.is_ascii(), "{shown}");
            assert!(
                shown.starts_with("x\\u{") && shown.ends_with("}y"),
                "{shown}"
            );
        }
        assert_eq!(name("\u{3042}"), Some("\\u{3042}".to_owned()));
        assert_eq!(
            name("claude\" (pid 1)"),
            Some("claude\\u{22} \\u{28}pid 1\\u{29}".to_owned())
        );
        assert_eq!(name("   "), None);
        let long = name(&"é".repeat(40)).unwrap();
        assert!(long.len() <= 64 && long.ends_with('}'), "{long}");
        assert_eq!(
            PeerProcess::new(7, Some("\u{202e}")).name(),
            Some("\\u{202e}")
        );
        assert_eq!(PeerProcess::new(7, None).name(), None);
    }

    #[test]
    fn plain_names_only() {
        assert!(is_plain_name("screenshot-1.png"));
        for bad in ["", ".", "..", "a/b", "../a", "a\\b", ".hidden", "a\0b"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }
}
