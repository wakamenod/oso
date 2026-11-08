//! The OS clipboard, read and written off the UI thread (ADR 0074).
//!
//! Must not depend on: `window` or any other module of this crate; the clipboard is a
//! thing of its own, beside the window layer.
//!
//! [`Clipboard::start`] starts two threads (`threads`). A worker owns the system's
//! clipboard and does what is asked of it in order; reading can wait on another process,
//! as the app that copied the text may be slow to hand it over. The front answers the
//! host: it gives each read an answer within [`READ_TIMEOUT`], [`Contents::NoAnswer`] if
//! the worker has none by then, so the host never waits longer, and holds writes while a
//! read is out. A read that comes while the worker is still on an earlier one is
//! answered at once with [`Contents::NoAnswer`], and a worker stuck past [`GIVE_UP`] is
//! left behind for a new one.
//!
//! The backend remembers the change count after its own last write, so a read first asks
//! only whether anything newer is there, and reads text only if so. Each write and each
//! text read gets a [`Fingerprint`]: its length and a hash keyed by a value made for this
//! clipboard, so the host can tell text that came back from another app without reading
//! it on the UI thread.
//!
//! Nothing here logs or prints text, and the types that carry it print only its length.

use std::fmt;
use std::io;
use std::sync::mpsc::Sender;
use std::time::Duration;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(test, feature = "dev"))]
mod memory;
#[cfg(not(target_os = "macos"))]
mod none;
mod threads;

use threads::{Factory, Message, Timing, WriteJob};

#[cfg(any(test, feature = "dev"))]
pub use memory::MemoryClipboard;

/// How long a read may take before the host is told [`Contents::NoAnswer`].
pub const READ_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a read may take before its worker is left behind and a new one takes over,
/// as Emacs's `x-selection-timeout`.
pub const GIVE_UP: Duration = Duration::from_secs(5);

/// Which clipboard to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The system's. On a system with no backend yet, there is none: a write does
    /// nothing, and a read finds nothing.
    System,
    /// One in memory, for tests and the end-to-end flows, so that they never touch the
    /// person's. Only dev builds have it.
    #[cfg(any(test, feature = "dev"))]
    Memory,
}

/// The length of a text and a keyed hash of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    /// The length in bytes.
    pub len: usize,
    /// The hash, keyed by a value that lives only in this clipboard.
    pub hash: u64,
}

/// What the clipboard held when the host asked.
#[derive(Clone, PartialEq, Eq)]
pub enum Contents {
    /// What this clipboard wrote last, with nothing newer from another app.
    Own,
    /// Text another app put there.
    Text {
        /// The text.
        text: String,
        /// The clipboard marks it concealed, as a password manager does.
        concealed: bool,
        /// Its fingerprint.
        fingerprint: Fingerprint,
    },
    /// No text, or no clipboard.
    Nothing,
    /// Text longer than the clipboard was started to read, which was not read.
    TooLarge,
    /// No answer within [`READ_TIMEOUT`], or a read still running from before.
    NoAnswer,
}

impl fmt::Debug for Contents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Own => f.write_str("Own"),
            // Never the text: it could reach a log.
            Self::Text {
                text, concealed, ..
            } => f
                .debug_struct("Text")
                .field("len", &text.len())
                .field("concealed", concealed)
                .finish_non_exhaustive(),
            Self::Nothing => f.write_str("Nothing"),
            Self::TooLarge => f.write_str("TooLarge"),
            Self::NoAnswer => f.write_str("NoAnswer"),
        }
    }
}

/// What the clipboard tells the host, from its own thread.
#[derive(Debug)]
pub enum Event {
    /// The text written for `id` is on the clipboard, with this fingerprint.
    Written {
        /// The id the host gave the write.
        id: u64,
        /// The text's fingerprint.
        fingerprint: Fingerprint,
    },
    /// The answer to a read.
    Read(Contents),
    /// A read did not return within [`GIVE_UP`]; a new worker has taken over.
    NotAnswering,
}

/// What a backend found on reading.
#[cfg_attr(
    not(any(target_os = "macos", test, feature = "dev")),
    expect(
        dead_code,
        reason = "only the macOS backend and the one in memory find text; Linux and Windows have none until their window layers (ADR 0074)"
    )
)]
enum Found {
    Text { text: String, concealed: bool },
    Nothing,
    TooLarge,
}

/// One system's clipboard. Its methods may wait on other processes, and run only on the
/// backend's thread.
trait Backend {
    /// A number that changes whenever anything writes the clipboard, if the system has
    /// one.
    fn change_count(&mut self) -> Option<i64>;
    /// Replaces what the clipboard holds with `text`, marked concealed for clipboard
    /// managers if `concealed`. Returns the change count after it.
    fn write(&mut self, text: &str, concealed: bool) -> Option<i64>;
    /// Reads the text, if there is any and it is at most `max` bytes.
    fn read(&mut self, max: usize) -> Found;
}

/// Text a write computes on the backend's thread, so that joining it costs the UI thread
/// nothing.
pub type MakeText = Box<dyn FnOnce() -> String + Send>;

/// The host's handle on the clipboard. Dropping it ends the threads once the worker has
/// done what it was asked.
pub struct Clipboard {
    messages: Sender<Message>,
    #[cfg(any(test, feature = "dev"))]
    memory: Option<MemoryClipboard>,
}

impl fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clipboard").finish_non_exhaustive()
    }
}

/// How long the front waits on a worker, as the host sees it.
const TIMING: Timing = Timing {
    answer: READ_TIMEOUT,
    give_up: GIVE_UP,
};

impl Clipboard {
    /// Starts the clipboard's threads over `source`, reading at most `max_read` bytes of
    /// text. `report` gets every [`Event`], on another thread.
    ///
    /// # Errors
    ///
    /// When a thread cannot be started.
    pub fn start(
        source: Source,
        max_read: usize,
        report: impl Fn(Event) + Send + 'static,
    ) -> io::Result<Self> {
        match source {
            #[cfg(any(test, feature = "dev"))]
            Source::Memory => {
                let memory = MemoryClipboard::default();
                let shared = memory.clone();
                let make: Factory = Box::new(move || Box::new(shared.clone()));
                let mut clipboard = Self::spawn(make, max_read, TIMING, report)?;
                clipboard.memory = Some(memory);
                Ok(clipboard)
            }
            Source::System => Self::spawn(Box::new(system), max_read, TIMING, report),
        }
    }

    fn spawn(
        make: Factory,
        max_read: usize,
        timing: Timing,
        report: impl Fn(Event) + Send + 'static,
    ) -> io::Result<Self> {
        Ok(Self {
            messages: threads::start(make, max_read, timing, report)?,
            #[cfg(any(test, feature = "dev"))]
            memory: None,
        })
    }

    /// Puts the text `text` makes on the clipboard, marked concealed if `concealed`, and
    /// reports [`Event::Written`] with `id`. Writes happen in order, and one not yet
    /// started when a newer one comes is dropped, with no report.
    pub fn write(&self, id: u64, concealed: bool, text: MakeText) {
        // A send fails only once the threads have ended, with the process.
        let _ = self.messages.send(Message::Write(WriteJob {
            id,
            concealed,
            text,
        }));
    }

    /// Reads the clipboard, and reports [`Event::Read`] within [`READ_TIMEOUT`].
    pub fn read(&self) {
        let _ = self.messages.send(Message::Read);
    }

    /// The clipboard in memory, if it was started over one.
    #[cfg(any(test, feature = "dev"))]
    pub fn memory(&self) -> Option<&MemoryClipboard> {
        self.memory.as_ref()
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        let _ = self.messages.send(Message::Close);
    }
}

#[cfg(target_os = "macos")]
fn system() -> Box<dyn Backend + Send> {
    Box::new(macos::Pasteboard)
}

#[cfg(not(target_os = "macos"))]
fn system() -> Box<dyn Backend + Send> {
    Box::new(none::NoClipboard)
}

#[cfg(test)]
mod tests;
