//! A terminal as the host holds it: a handle to the thread that owns the PTY, the child and
//! the engine (ADR 0120, Q4). Nothing here blocks: input and requests queue, and the UI takes
//! a snapshot when one is ready. At most one snapshot is in flight, so bulk output costs the UI
//! one snapshot per frame. Keys and pastes are encoded on the thread, against the modes the
//! program has set by then.
//!
//! Must not depend on: anything the UI draws with; the thread's work is in `thread`.

#[cfg(unix)]
mod thread;

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use oso_core::key::KeyPress;
use oso_platform::process::Executable;
use oso_platform::process::pty::{self, Size, Waker};

use crate::input;
use crate::limits;
use crate::snapshot::Pointer;
use crate::snapshot::{Palette, Scroll, Snapshot};

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

/// `Terminal::send`, `send_key`, `send_pointer` or `paste` refused input: 16 MiB wait to be
/// written already, counting what queued keys, pointers and pastes may take.
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
    Input(Input),
    Scroll(Scroll),
    Resize(Size),
    Palette(Palette),
    Recycle(Snapshot),
    HangUp,
}

/// Input for the program, as the UI gave it; the thread encodes it.
enum Input {
    Bytes(Vec<u8>),
    Key(KeyPress),
    Pointer(Pointer),
    Paste { text: String, lines_allowed: bool },
    Focus(bool),
}

impl Input {
    /// What of the input queue this holds from the handle until its thread has encoded it.
    /// A focus report holds nothing: the thread takes its 3 bytes, or drops it when the queue
    /// is full.
    fn reserved(&self) -> usize {
        match self {
            Input::Bytes(bytes) => bytes.len(),
            Input::Key(_) | Input::Pointer(_) => limits::KEY_BYTES_MAX,
            Input::Paste { text, .. } => input::paste_bound(text.len()),
            Input::Focus(_) => 0,
        }
    }
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
    /// The child's process id while it runs and is not reaped; 0 before it starts and once
    /// the thread reaps it, or leaves it to a reaper.
    pid: AtomicU32,
}

impl Shared {
    /// Holds `n` bytes of the input queue, unless 16 MiB would wait.
    fn reserve(&self, n: usize) -> Result<(), QueueFull> {
        self.queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                queued
                    .checked_add(n)
                    .filter(|&total| total <= limits::INPUT_QUEUE_MAX)
            })
            .map(drop)
            .map_err(|_| QueueFull)
    }

    /// Gives back `n` bytes held: written, or never needed.
    fn release(&self, n: usize) {
        self.queued.fetch_sub(n, Ordering::AcqRel);
    }
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
        Terminal::start_holding(start, wake, limits::RENDER_HOLD_MAX)
    }

    /// `start`, with synchronized updates held back for at most `hold_limit`; tests shorten it.
    fn start_holding(
        start: Start,
        wake: Box<dyn Fn() + Send>,
        hold_limit: Duration,
    ) -> io::Result<Terminal> {
        let (waker, wakeup) = pty::wake_pair()?;
        let (requests, requests_rx) = mpsc::channel();
        let (snapshots_tx, snapshots) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        spawn_thread(
            start,
            hold_limit,
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

    /// The snapshot that is ready, if one is. The next is built only after this, once
    /// [`Terminal::recycle`] gives this one back, or at the thread's next wake.
    pub fn take_snapshot(&self) -> Option<Snapshot> {
        let snapshot = self.snapshots.try_recv().ok()?;
        self.shared.in_flight.store(false, Ordering::Release);
        Some(snapshot)
    }

    /// Gives a snapshot's buffers back, so that later snapshots need not allocate, and wakes
    /// the thread to build the next in them: one wake per snapshot taken.
    pub fn recycle(&self, snapshot: Snapshot) {
        if self.requests.send(Request::Recycle(snapshot)).is_ok() {
            self.wake();
        }
    }

    /// Queues bytes for the program, already encoded, and scrolls the view back to the
    /// bottom when the thread takes them. Refused while 16 MiB wait.
    pub fn send(&self, bytes: Vec<u8>) -> Result<(), QueueFull> {
        self.input(Input::Bytes(bytes))
    }

    /// Queues a key for the program. Its thread encodes it against the modes the program has
    /// set by then (the cursor keys, the kitty keyboard flags, Alt's ESC prefix), with the
    /// library's encoder; Meta, Command on macOS, is sent as Alt. A key that sends anything
    /// scrolls the view back to the bottom. Each key holds 128 bytes of the 16 MiB queue
    /// until it is encoded and then only what it took, so this is refused once less than
    /// 128 bytes are free.
    pub fn send_key(&self, key: KeyPress) -> Result<(), QueueFull> {
        self.input(Input::Key(key))
    }

    /// Queues the mouse's `pointer` for the program. Its thread encodes it against the modes
    /// the program has set by then: a report in the mode and format it asked for (9, 1000,
    /// 1002, 1003; 1005, 1006, 1015, 1016), or nothing while it asked for none. The wheel's
    /// `↑` and `↓` of mode 1007 are the caller's to send, as keys. One that sends anything
    /// scrolls the view back to the bottom. Each holds 128 bytes of the 16 MiB
    /// queue until it is encoded, as a key does.
    pub fn send_pointer(&self, pointer: Pointer) -> Result<(), QueueFull> {
        self.input(Input::Pointer(pointer))
    }

    /// Queues a paste. Its thread turns CRLF and CR into LF and removes every other control
    /// character but TAB, so a paste cannot end its own bracket; then, with bracketed paste on
    /// as the program has it by then, sends it in brackets. With it off, each LF is sent as
    /// CR, and a text with a line break is sent only if `lines_allowed` (the person said
    /// yes); otherwise nothing is sent and a snapshot carries `Event::PasteRefused`. A paste
    /// that sends anything scrolls the view back to the bottom. It holds the text's length
    /// and 12 bytes of the 16 MiB queue until it is encoded.
    pub fn paste(&self, text: String, lines_allowed: bool) -> Result<(), QueueFull> {
        self.input(Input::Paste {
            text,
            lines_allowed,
        })
    }

    /// Tells the program the terminal gained or lost focus (`CSI I`, `CSI O`), only while it
    /// has focus reports (mode 1004) on, and only on a change. Dropped when the input queue
    /// is full.
    pub fn set_focused(&self, focused: bool) {
        self.request(Request::Input(Input::Focus(focused)));
    }

    /// Moves the view over the scrollback; the next snapshot sends the rows scrolled to and
    /// `Snapshot::scrolled`. Output while scrolled back keeps the view on the same lines.
    pub fn scroll(&self, scroll: Scroll) {
        self.request(Request::Scroll(scroll));
    }

    fn input(&self, input: Input) -> Result<(), QueueFull> {
        self.shared.reserve(input.reserved())?;
        self.request(Request::Input(input));
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

    /// The child's process id, from its start until it is reaped; `None` before and after.
    /// It is cleared before the reap, so the id of a process that ended, which the system may
    /// give to another, is never returned (ADR 0121).
    pub fn pid(&self) -> Option<u32> {
        match self.shared.pid.load(Ordering::Acquire) {
            0 => None,
            pid => Some(pid),
        }
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
    hold_limit: Duration,
    wakeup: pty::Wakeup,
    requests: mpsc::Receiver<Request>,
    snapshots: mpsc::Sender<Snapshot>,
    shared: Arc<Shared>,
    wake: Box<dyn Fn() + Send>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("oso-terminal".into())
        .spawn(move || {
            let channels = thread::Channels {
                wakeup: &wakeup,
                requests: &requests,
                snapshots: &snapshots,
                shared: &shared,
                wake: &*wake,
            };
            thread::run(start, hold_limit, channels);
        })
        .map(drop)
}

#[cfg(not(unix))]
fn spawn_thread(
    _start: Start,
    _hold_limit: Duration,
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
