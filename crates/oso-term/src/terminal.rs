//! A terminal as the host holds it: a handle to the thread that owns the PTY, the child and
//! the engine (ADR 0120, Q4). Nothing here blocks: input and requests queue, and the UI takes
//! a snapshot when one is ready. At most one snapshot is in flight, so bulk output costs the UI
//! one snapshot per frame.
//!
//! Must not depend on: anything the UI draws with; the thread's work is in `thread`.

#[cfg(unix)]
mod thread;

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use oso_platform::process::Executable;
use oso_platform::process::pty::{self, Size, Waker};

use crate::limits;
use crate::snapshot::{Palette, Snapshot};

/// What a terminal starts: its child, as `oso_platform::process::spawn` takes it, and the
/// terminal's size, scrollback and colors.
#[derive(Clone, Debug)]
pub struct Start {
    pub executable: Executable,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub project: Option<PathBuf>,
    /// Environment names passed through, beyond the process module's allowlist.
    pub inherit: Vec<String>,
    /// Environment variables set by value.
    pub set: Vec<(String, OsString)>,
    pub size: Size,
    /// Lines, at most `limits::SCROLLBACK_MAX`.
    pub scrollback: usize,
    pub palette: Palette,
}

/// `Terminal::send` refused input: 16 MiB wait to be written already.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueFull;

impl fmt::Display for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the terminal's input queue is full")
    }
}

impl Error for QueueFull {}

/// What the UI asks of a terminal's thread.
enum Request {
    Input(Vec<u8>),
    Resize(Size),
    Palette(Palette),
    Recycle(Snapshot),
    HangUp,
}

/// The state the handle and the thread share, besides their channels.
#[derive(Default)]
struct Shared {
    /// A snapshot was sent and not taken yet.
    in_flight: AtomicBool,
    /// Input bytes queued and not written yet.
    queued: AtomicUsize,
    /// A request waits: the thread cuts its parse short to serve it.
    requested: AtomicBool,
}

/// A running terminal. Dropping it hangs the terminal up and lets its thread finish, without
/// waiting for it.
pub struct Terminal {
    requests: mpsc::Sender<Request>,
    snapshots: mpsc::Receiver<Snapshot>,
    waker: Waker,
    shared: Arc<Shared>,
}

impl Terminal {
    /// Starts the terminal's thread, which opens the PTY, starts the child in it and reads
    /// it. Returns at once; a child that fails to start ends the terminal with
    /// `End::Failed`. `wake` is called from that thread whenever a snapshot is ready. On
    /// Windows, which has no terminals yet (ADR 0120, Q3), this returns `Unsupported`.
    pub fn start(start: Start, wake: Box<dyn Fn() + Send>) -> io::Result<Terminal> {
        let (waker, wakeup) = pty::wake_pair()?;
        let (requests, requests_rx) = mpsc::channel();
        let (snapshots_tx, snapshots) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        spawn_thread(
            start,
            wakeup,
            requests_rx,
            snapshots_tx,
            Arc::clone(&shared),
            wake,
        )?;
        Ok(Terminal {
            requests,
            snapshots,
            waker,
            shared,
        })
    }

    /// The snapshot that is ready, if one is. The next is built only after this.
    pub fn take_snapshot(&self) -> Option<Snapshot> {
        let snapshot = self.snapshots.try_recv().ok()?;
        self.shared.in_flight.store(false, Ordering::Release);
        self.wake();
        Some(snapshot)
    }

    /// Gives a snapshot's buffers back, so that later snapshots need not allocate.
    pub fn recycle(&self, snapshot: Snapshot) {
        let _ = self.requests.send(Request::Recycle(snapshot));
    }

    /// Queues bytes for the program, already encoded. Refused while 16 MiB wait; nothing
    /// is queued for no bytes.
    pub fn send(&self, bytes: Vec<u8>) -> Result<(), QueueFull> {
        let len = bytes.len();
        if len == 0 {
            return Ok(());
        }
        self.shared
            .queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued + len <= limits::INPUT_QUEUE_MAX).then_some(queued + len)
            })
            .map_err(|_| QueueFull)?;
        self.request(Request::Input(bytes));
        Ok(())
    }

    /// Resizes the PTY and the grid, clamped to 4 × 1 cells at least and 1000 × 500 at most.
    pub fn resize(&self, size: Size) {
        self.request(Request::Resize(size));
    }

    /// The theme's colors, after a theme switch.
    pub fn set_palette(&self, palette: Palette) {
        self.request(Request::Palette(palette));
    }

    /// Sends `SIGHUP` to the terminal's foreground process group, as closing a terminal does.
    pub fn hang_up(&self) {
        self.request(Request::HangUp);
    }

    fn request(&self, request: Request) {
        self.shared.requested.store(true, Ordering::Release);
        if self.requests.send(request).is_ok() {
            self.wake();
        }
    }

    fn wake(&self) {
        // A full wake-up pipe already holds a wake the thread has not seen.
        let _ = self.waker.wake();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The thread sees its requests end, hangs up and finishes.
        let _ = self.requests.send(Request::HangUp);
        self.wake();
    }
}

impl fmt::Debug for Terminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Terminal").finish_non_exhaustive()
    }
}

#[cfg(unix)]
fn spawn_thread(
    start: Start,
    wakeup: pty::Wakeup,
    requests: mpsc::Receiver<Request>,
    snapshots: mpsc::Sender<Snapshot>,
    shared: Arc<Shared>,
    wake: Box<dyn Fn() + Send>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("oso-terminal".into())
        .spawn(move || thread::run(start, &wakeup, &requests, &snapshots, &shared, &*wake))
        .map(drop)
}

#[cfg(not(unix))]
fn spawn_thread(
    _start: Start,
    _wakeup: pty::Wakeup,
    _requests: mpsc::Receiver<Request>,
    _snapshots: mpsc::Sender<Snapshot>,
    _shared: Arc<Shared>,
    _wake: Box<dyn Fn() + Send>,
) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod tests;
