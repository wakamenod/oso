//! A terminal over a restart (ADR 0120, step 11; ADR 0133), as the host asks for it. The
//! instance going stops reading each PTY and writes its terminal's state; the one coming
//! restores an engine from that state and reads the PTY once its master arrives. So at any
//! time one instance reads each PTY, and what the program writes meanwhile waits in the
//! kernel.
//!
//! Must not depend on: the control socket or how the state and the master travel; the host
//! carries them.

use std::fmt;
use std::io;
#[cfg(unix)]
use std::os::fd::OwnedFd;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::SystemTime;

use oso_platform::process::pty::Size;

use super::Terminal;
use crate::snapshot::Palette;

/// A descriptor; there are none to pass where terminals do not exist yet.
#[cfg(not(unix))]
pub type OwnedFd = std::convert::Infallible;

/// How a terminal's state is written for the next instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// The library's snapshot: both screens, every mode, a sequence the program had not
    /// finished. Only an instance built with the same library reads it.
    Snapshot,
    /// Escape sequences that redraw the screen in use and its scrollback, for an instance
    /// built with another library.
    Escapes,
}

/// What a terminal's thread wrote for the next instance.
pub struct Exported {
    /// The bytes that restore the terminal, written as `encoding`.
    pub state: Vec<u8>,
    pub encoding: Encoding,
    /// A second descriptor for the terminal's master, close-on-exec.
    pub master: OwnedFd,
    /// The child's process id, if it runs.
    pub pid: Option<u32>,
    /// When the child started, as the system says: what tells it from a later process with
    /// the same id once it is no longer this instance's child.
    pub started: Option<SystemTime>,
    pub size: Size,
    /// The program reads a password (ADR 0120, Q17).
    pub echo_off: bool,
}

impl fmt::Debug for Exported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the state: it is the terminal's text.
        f.debug_struct("Exported")
            .field("bytes", &self.state.len())
            .field("encoding", &self.encoding)
            .field("pid", &self.pid)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Why a terminal was not exported. It reads on as before.
#[derive(Debug)]
pub enum NotExported {
    /// Its state takes more than the bytes it was given.
    TooLarge,
    /// The terminal has ended.
    Ended,
    Failed(io::Error),
}

/// A terminal the instance before exported, to take over: what `Exported` gave, but the
/// master, which [`Terminal::attach`] brings.
pub struct Adopt {
    pub state: Vec<u8>,
    pub encoding: Encoding,
    pub size: Size,
    /// Lines, at most `limits::SCROLLBACK_MAX`.
    pub scrollback: usize,
    pub palette: Palette,
    pub pid: Option<u32>,
    pub started: Option<SystemTime>,
}

impl fmt::Debug for Adopt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Adopt")
            .field("bytes", &self.state.len())
            .field("encoding", &self.encoding)
            .field("pid", &self.pid)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// What the host asks of a terminal's thread for a restart.
pub(super) enum Request {
    Export {
        library: String,
        budget: Arc<AtomicUsize>,
        reply: mpsc::Sender<Result<Exported, NotExported>>,
    },
    Resume,
    Release,
    Attach(OwnedFd),
    Abandon,
}

impl Terminal {
    /// The name of the library and its snapshot format, which an instance gives the one it
    /// replaces: a terminal exported for this name writes a snapshot, for another one escape
    /// sequences.
    pub fn library() -> String {
        #[cfg(unix)]
        let library = crate::engine::ghostty::carry::library();
        #[cfg(not(unix))]
        let library = String::new();
        library
    }

    /// Stops reading the PTY and writes the terminal's state for the next instance, taking its
    /// bytes from `budget`, which the terminals exported for the same restart share, as escape
    /// sequences: the library's snapshot, for an instance with the same [`Terminal::library`],
    /// is off until a patch to the library. Input still goes to the program, and what it writes waits in the kernel.
    /// The answer comes on the receiver, and the thread's `wake` is called. Reading starts
    /// again at [`Terminal::resume`], or by itself after `limits::PAUSE_MAX`; either counts
    /// in [`Terminal::resumes`].
    pub fn export(
        &self,
        library: &str,
        budget: Arc<AtomicUsize>,
    ) -> mpsc::Receiver<Result<Exported, NotExported>> {
        let (reply, answer) = mpsc::channel();
        let request = Request::Export {
            library: library.to_owned(),
            budget,
            reply,
        };
        self.request(super::Request::Handoff(request));
        answer
    }

    /// Reads the PTY again after an export, as the restart did not happen.
    pub fn resume(&self) {
        self.request(super::Request::Handoff(Request::Resume));
    }

    /// How many times the thread has read again after an export.
    pub fn resumes(&self) -> u64 {
        self.shared.resumes.load(Ordering::Acquire)
    }

    /// Ends the terminal's thread without hanging it up or reaping its child, as the next
    /// instance has its master. Nothing else is served after it.
    pub fn release(self) {
        self.request(super::Request::Handoff(Request::Release));
    }

    /// Starts the thread of a terminal the instance before exported. It restores the engine
    /// from `adopt` and sends its first snapshot, then reads nothing until
    /// [`Terminal::attach`] brings the master; what is asked meanwhile waits, input up to the
    /// queue's limit. Its child is not this instance's: its end comes from the PTY's end,
    /// with no status. A state the library refuses ends it with `End::Failed`.
    pub fn adopt(adopt: Adopt, wake: Box<dyn Fn() + Send>) -> io::Result<Terminal> {
        #[cfg(unix)]
        let work = move |channels: super::thread::Channels<'_>| {
            super::thread::adopt(adopt, crate::limits::RENDER_HOLD_MAX, channels);
        };
        #[cfg(not(unix))]
        let work = move |_: ()| drop(adopt);
        Terminal::spawn(work, wake)
    }

    /// Brings an adopted terminal its master, once the instance before has gone. A
    /// descriptor that is no terminal's master ends it with `End::Failed`.
    pub fn attach(&self, master: OwnedFd) {
        self.request(super::Request::Handoff(Request::Attach(master)));
    }

    /// Ends an adopted terminal whose master will not come, as the instance before kept it:
    /// its end is `End::Failed`, and its last screen stays.
    pub fn abandon(&self) {
        self.request(super::Request::Handoff(Request::Abandon));
    }
}
