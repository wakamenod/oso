//! The OS clipboard, read and written off the UI thread (ADR 0074).
//!
//! Must not depend on: `window` or any other module of this crate; the clipboard is a
//! thing of its own, beside the window layer.
//!
//! [`Clipboard::start`] starts two threads. The backend's owns the system's clipboard
//! and does what is asked of it in order; reading can wait on another process, as the
//! app that copied the text may be slow to hand it over. The other answers the host:
//! it hands work to the backend and gives each read an answer within
//! [`READ_TIMEOUT`], [`Contents::NoAnswer`] if the backend has none by then, so the host
//! never waits longer. A read that comes while the backend is still on an earlier one
//! is answered at once with [`Contents::NoAnswer`].
//!
//! The backend remembers the change count after its own last write, so a read first asks
//! only whether anything newer is there, and reads text only if so. Each write and each
//! text read gets a [`Fingerprint`]: its length and a hash keyed by a value made for this
//! clipboard, so the host can tell text that came back from another app without reading
//! it on the UI thread.
//!
//! Nothing here logs or prints text, and the types that carry it print only its length.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::BuildHasher;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(test, feature = "dev"))]
mod memory;
#[cfg(not(target_os = "macos"))]
mod none;

#[cfg(any(test, feature = "dev"))]
pub use memory::MemoryClipboard;

/// How long a read may take before the host is told [`Contents::NoAnswer`].
pub const READ_TIMEOUT: Duration = Duration::from_millis(500);

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
}

/// What a backend found on reading.
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

enum Job {
    Write {
        id: u64,
        concealed: bool,
        text: MakeText,
    },
    Read,
}

enum Message {
    Write {
        id: u64,
        concealed: bool,
        text: MakeText,
    },
    Read,
    Written {
        id: u64,
        fingerprint: Fingerprint,
    },
    Found(Contents),
    /// The host's handle went: end once the backend has done its work.
    Close,
}

/// The host's handle on the clipboard. Dropping it ends the threads once the backend has
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
                let mut clipboard = Self::spawn(Box::new(memory.clone()), max_read, report)?;
                clipboard.memory = Some(memory);
                Ok(clipboard)
            }
            Source::System => Self::spawn(system(), max_read, report),
        }
    }

    fn spawn(
        backend: Box<dyn Backend + Send>,
        max_read: usize,
        report: impl Fn(Event) + Send + 'static,
    ) -> io::Result<Self> {
        let (messages, inbox) = mpsc::channel();
        let (jobs, work) = mpsc::channel();
        let keys = RandomState::new();
        let answers = messages.clone();
        thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || run_backend(backend, max_read, &keys, &work, &answers))?;
        thread::Builder::new()
            .name("clipboard answers".into())
            .spawn(move || run_front(&inbox, &jobs, &report))?;
        Ok(Self {
            messages,
            #[cfg(any(test, feature = "dev"))]
            memory: None,
        })
    }

    /// Puts the text `text` makes on the clipboard, marked concealed if `concealed`, and
    /// reports [`Event::Written`] with `id`. Writes happen in order, and one not yet
    /// started when a newer one comes is dropped, with no report.
    pub fn write(&self, id: u64, concealed: bool, text: MakeText) {
        // A send fails only once the threads have ended, with the process.
        let _ = self.messages.send(Message::Write {
            id,
            concealed,
            text,
        });
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

/// The backend's thread: does each job in order, and sends what came of it.
fn run_backend(
    mut backend: Box<dyn Backend + Send>,
    max_read: usize,
    keys: &RandomState,
    work: &Receiver<Job>,
    answers: &Sender<Message>,
) {
    let fingerprint = |text: &str| Fingerprint {
        len: text.len(),
        hash: keys.hash_one(text),
    };
    let mut own = None;
    // A job taken from the queue while looking for a newer write.
    let mut next = None;
    loop {
        let Some(job) = next.take().or_else(|| work.recv().ok()) else {
            return;
        };
        let message = match job {
            Job::Write {
                mut id,
                mut concealed,
                mut text,
            } => {
                // A newer write replaces one not yet done, so kills in a row cost one
                // write, and a read waits behind one at most.
                while let Ok(later) = work.try_recv() {
                    match later {
                        Job::Write {
                            id: newer,
                            concealed: marked,
                            text: newer_text,
                        } => (id, concealed, text) = (newer, marked, newer_text),
                        read @ Job::Read => {
                            next = Some(read);
                            break;
                        }
                    }
                }
                let text = text();
                own = backend.write(&text, concealed);
                Message::Written {
                    id,
                    fingerprint: fingerprint(&text),
                }
            }
            Job::Read => Message::Found(match backend.change_count() {
                Some(count) if Some(count) == own => Contents::Own,
                _ => match backend.read(max_read) {
                    Found::Text { text, concealed } => Contents::Text {
                        fingerprint: fingerprint(&text),
                        text,
                        concealed,
                    },
                    Found::Nothing => Contents::Nothing,
                    Found::TooLarge => Contents::TooLarge,
                },
            }),
        };
        if answers.send(message).is_err() {
            return;
        }
    }
}

/// The thread that answers the host: hands work to the backend, and answers each read in
/// time.
fn run_front(inbox: &Receiver<Message>, jobs: &Sender<Job>, report: &impl Fn(Event)) {
    // The deadline of the read the backend is on, until it is answered or too late.
    let mut deadline: Option<Instant> = None;
    // The backend is on a read, answered or not.
    let mut reading = false;
    loop {
        let message = match deadline {
            Some(at) => match inbox.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => {
                    deadline = None;
                    report(Event::Read(Contents::NoAnswer));
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match inbox.recv() {
                Ok(message) => message,
                Err(_) => return,
            },
        };
        match message {
            Message::Write {
                id,
                concealed,
                text,
            } => {
                if jobs
                    .send(Job::Write {
                        id,
                        concealed,
                        text,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Message::Read if reading => report(Event::Read(Contents::NoAnswer)),
            Message::Read => {
                if jobs.send(Job::Read).is_err() {
                    return;
                }
                reading = true;
                deadline = Some(Instant::now() + READ_TIMEOUT);
            }
            Message::Written { id, fingerprint } => report(Event::Written { id, fingerprint }),
            // Dropping `jobs` ends the backend's thread once it has done its work.
            Message::Close => return,
            Message::Found(contents) => {
                reading = false;
                // Answered already, if its time ran out.
                if deadline.take().is_some() {
                    report(Event::Read(contents));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
