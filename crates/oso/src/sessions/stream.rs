//! A native session's `claude -p` once it runs (ADR 0165): its stream read on threads of its
//! own and handed on in batches, its first message written, and its stop.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; the start thread starts the process
//! and gives it here, and what is read goes back through the function the app gives.
//!
//! - `oso-native-read` reads standard output in lines of at most [`MAX_LINE`] bytes; a longer
//!   one is unreadable, and the reader goes on from its end. Each line is read into an event
//!   there, off the UI thread. Once standard output ends and the process has ended, what is
//!   left of its group, such as a job its tools left in the background, is sent `SIGTERM`
//!   then `SIGKILL`, and only then is the process reaped and its end said, with the end of
//!   what it wrote on standard error. So its group can be reached until then.
//! - `oso-native-batch` hands the events on at most [`BATCHES_PER_SECOND`] times a second, at
//!   most [`MAX_BATCH`] at a time; a full queue holds the reader back, and so the process.
//! - `oso-native-err` keeps the last [`MAX_ERR`] bytes of standard error; nothing of it is
//!   logged, as it may quote the conversation.
//! - `oso-native-write` writes the person's messages on standard input, in order, from a
//!   queue of at most [`MAX_WAITING`] lines: [`Stream::send`] never waits, so a process that
//!   does not read holds back no one but itself, and a full queue refuses the message (ADR
//!   0165, step 2). The control layer's `initialize` goes first, then the first message, the
//!   same way, once the readers run, so that a process that writes before it reads never
//!   waits on oso; its requests share the queue, in order with the messages (step 2c). If
//!   anything fails once the process runs, its group is killed and it is reaped.
//! - An interrupt by signal (`SIGINT`) reaches the process alone: the CLI ends its own tools,
//!   as a terminal's `C-c`, a byte, would have it do.
//! - A stop closes standard input and sends the group `SIGTERM` at once, and `SIGKILL` after
//!   [`STOP_WAIT`] if it has not ended. oso's exit kills the group at once.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Child, Signal};

use super::native::wire::control::{self, INITIALIZE, Request};
use super::native::wire::{self, Event, MAX_LINE};

/// How often a batch of events goes to the UI thread, at most.
pub const BATCHES_PER_SECOND: u32 = 30;
/// The most events one batch holds.
pub const MAX_BATCH: usize = 1024;
/// The most of standard error kept, its end.
pub const MAX_ERR: usize = 16 << 10;
/// How long a stop waits before `SIGKILL`.
pub const STOP_WAIT: Duration = Duration::from_secs(5);
/// How long what is left of an ended process's group has after `SIGTERM`.
const GROUP_WAIT: Duration = Duration::from_millis(200);
/// How many events wait for the batcher before the reader waits.
const QUEUE: usize = 1024;
/// How often a process that closed its output is looked at until it ends.
const EXIT_POLL: Duration = Duration::from_millis(20);
/// The most lines waiting for the writer.
pub const MAX_WAITING: usize = 16;
/// The longest message the person sends a session, in bytes: a brief's (ADR 0121) and a
/// composer's (ADR 0165, step 2).
pub const MAX_MESSAGE: usize = 64 << 10;

/// Why a message was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotSent {
    /// [`MAX_WAITING`] lines wait already: the process does not read.
    Full,
    /// Its input is closed: the process stops, or could not be written to.
    Closed,
}

/// What the threads hand on.
#[derive(Debug)]
pub enum Out {
    /// Events read, in order.
    Events(Vec<Event>),
    /// The process ended, with its status if it exited, and the end of its standard error.
    Ended { status: Option<i32>, stderr: String },
}

/// What a native session's threads hand their reports to.
pub type Deliver = Box<dyn FnMut(Out) + Send>;

/// A native session's running process.
#[derive(Debug)]
pub struct Stream {
    child: Arc<Mutex<Child>>,
    /// The writer's queue, until standard input closes.
    input: Mutex<Option<SyncSender<String>>>,
    /// The process was reaped: its id may be another's now.
    reaped: Arc<AtomicBool>,
    pid: u32,
}

impl Stream {
    /// Reads `child`'s stream on threads of their own, handing what they read to `deliver`,
    /// then writes `initialize` and `first` as the person's first message, if any. `child` was started with
    /// piped standard input, output and error, in a session of its own. On any error, its group
    /// is killed and it is reaped.
    pub fn run(mut child: Child, first: Option<&str>, deliver: Deliver) -> std::io::Result<Self> {
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            end(&mut child);
            return Err(std::io::Error::other(
                "a pipe of the session's process is missing",
            ));
        };
        let pid = child.id();
        let child = Arc::new(Mutex::new(child));
        let reaped = Arc::new(AtomicBool::new(false));
        let (input, lines) = mpsc::sync_channel(MAX_WAITING);
        let stream = Self {
            child: Arc::clone(&child),
            input: Mutex::new(Some(input)),
            reaped: Arc::clone(&reaped),
            pid,
        };
        let err = Arc::new(Mutex::new(Vec::new()));
        let (queue, events) = mpsc::sync_channel(QUEUE);
        let started = named("oso-native-batch", move || batch(&events, deliver))
            .and_then(|()| {
                let kept = Arc::clone(&err);
                named("oso-native-err", move || keep_end(stderr, &kept))
            })
            .and_then(|()| {
                named("oso-native-read", move || {
                    read(stdout, &queue, &child, &reaped, &err);
                })
            })
            .and_then(|()| named("oso-native-write", move || write(stdin, &lines)))
            .and_then(|()| {
                (stream.request(INITIALIZE, Request::Initialize))
                    .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
            })
            .and_then(|()| match first {
                Some(first) => {
                    (stream.send(first)).map_err(|_| std::io::ErrorKind::BrokenPipe.into())
                }
                None => Ok(()),
            });
        if let Err(e) = started {
            stream.kill();
            return Err(e);
        }
        Ok(stream)
    }

    /// Hands `text` to the writer as the person's message, never waiting: refused if the
    /// writer's queue is full or the input closed.
    pub fn send(&self, text: &str) -> Result<(), NotSent> {
        self.offer_line(wire::user_message(text))
    }

    /// Hands the control request `request`, with id `id`, to the writer, never waiting:
    /// refused as [`Stream::send`] refuses.
    pub fn request(&self, id: u64, request: Request) -> Result<(), NotSent> {
        self.offer_line(control::request(id, request))
    }

    /// Hands `line` to the writer, never waiting.
    fn offer_line(&self, line: String) -> Result<(), NotSent> {
        let input = self.input.lock().unwrap_or_else(PoisonError::into_inner);
        offer(input.as_ref().ok_or(NotSent::Closed)?, line)
    }

    /// Interrupts the process with `SIGINT`, the process alone: the CLI ends the turn, then
    /// itself (ADR 0165, measured).
    pub fn interrupt(&self) {
        let _ = lock(&self.child).interrupt();
    }

    /// Closes standard input once the writer has written what waits: the writer's queue goes.
    fn close_input(&self) {
        drop(
            self.input
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    }

    /// The process's id, until it is reaped.
    pub fn pid(&self) -> Option<u32> {
        (!self.reaped.load(Ordering::Acquire)).then_some(self.pid)
    }

    /// Stops the process at the person's word: its input closes and its group is sent
    /// `SIGTERM` at once, then `SIGKILL` after [`STOP_WAIT`] if it has not ended, on a thread of
    /// its own.
    pub fn stop(&self) {
        self.close_input();
        let _ = lock(&self.child).signal_group(Signal::Terminate);
        let child = Arc::clone(&self.child);
        let stopping = named("oso-native-stop", move || {
            if !ended_within(&child, STOP_WAIT) {
                let _ = lock(&child).signal_group(Signal::Kill);
            }
        });
        if stopping.is_err() {
            self.kill();
        }
    }

    /// Kills the process's group at once, as oso exits.
    pub fn kill(&self) {
        self.close_input();
        let mut child = lock(&self.child);
        let _ = child.signal_group(Signal::Kill);
        let _ = child.kill();
    }
}

fn lock(child: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Kills `child`'s group and reaps it.
fn end(child: &mut Child) {
    let _ = child.signal_group(Signal::Kill);
    let _ = child.kill();
    let _ = child.wait();
}

fn named(name: &str, run: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(run)
        .map(drop)
}

/// Whether `child` ended within `wait`, looked at without reaping it.
fn ended_within(child: &Mutex<Child>, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    loop {
        if lock(child).exited().unwrap_or(true) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        thread::sleep(EXIT_POLL);
    }
}

/// Puts `line` on the writer's queue `input`, never waiting.
fn offer(input: &SyncSender<String>, line: String) -> Result<(), NotSent> {
    input.try_send(line).map_err(|e| match e {
        mpsc::TrySendError::Full(_) => NotSent::Full,
        mpsc::TrySendError::Disconnected(_) => NotSent::Closed,
    })
}

/// Writes each line of `lines` on `stdin` until the queue goes, as the input closes, or a
/// write fails, as when the process has ended; then `stdin` closes.
fn write(mut stdin: impl Write, lines: &Receiver<String>) {
    while let Ok(line) = lines.recv() {
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            return;
        }
    }
}

/// What the reader gives the batcher.
enum Item {
    Event(Event),
    Ended { status: Option<i32>, stderr: String },
}

/// Reads lines from `stdout` until it ends; then, once the process ended, ends what is left
/// of its group, reaps it and says how it ended.
fn read(
    stdout: impl std::io::Read,
    queue: &SyncSender<Item>,
    child: &Mutex<Child>,
    reaped: &AtomicBool,
    err: &Mutex<Vec<u8>>,
) {
    let mut reader = BufReader::with_capacity(64 << 10, stdout);
    let mut line = Vec::new();
    let mut sending = true;
    while sending {
        let item = match next_line(&mut reader, &mut line) {
            Ok(Some(None)) => Item::Event(wire::parse(&line)),
            Ok(Some(Some(bytes))) => Item::Event(Event::Unreadable { bytes }),
            Ok(None) | Err(_) => break,
        };
        sending = queue.send(item).is_ok();
    }
    while !lock(child).exited().unwrap_or(true) {
        thread::sleep(EXIT_POLL);
    }
    // What its tools left running goes with it, as a terminal's would with its hang-up.
    let _ = lock(child).signal_group(Signal::Terminate);
    thread::sleep(GROUP_WAIT);
    let status = {
        let mut child = lock(child);
        let _ = child.signal_group(Signal::Kill);
        reaped.store(true, Ordering::Release);
        child.wait().ok().and_then(|status| status.code())
    };
    let stderr =
        String::from_utf8_lossy(&err.lock().unwrap_or_else(PoisonError::into_inner)).into_owned();
    let _ = queue.send(Item::Ended { status, stderr });
}

/// The next line into `line`, without its `\n`: `Some(None)` when it fits in [`MAX_LINE`],
/// `Some(Some(length))` for one past it, whose bytes are skipped, and `None` at the end.
fn next_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
) -> std::io::Result<Option<Option<usize>>> {
    line.clear();
    let mut length = 0_usize;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((length > 0).then_some((length > MAX_LINE).then_some(length)));
        }
        let (part, done) = match available.iter().position(|byte| *byte == b'\n') {
            Some(at) => (&available[..at], at + 1),
            None => (available, available.len()),
        };
        length = length.saturating_add(part.len());
        if length <= MAX_LINE {
            line.extend_from_slice(part);
        } else {
            line.clear();
        }
        let ended = done > part.len();
        reader.consume(done);
        if ended {
            return Ok(Some((length > MAX_LINE).then_some(length)));
        }
    }
}

/// Hands the reader's events on in batches, at most [`BATCHES_PER_SECOND`] a second and
/// [`MAX_BATCH`] events each. If the reader is gone without saying the process ended, the end
/// is said as one with no status.
fn batch(events: &Receiver<Item>, mut deliver: Deliver) {
    let every = Duration::from_secs(1) / BATCHES_PER_SECOND;
    let mut last: Option<Instant> = None;
    let mut batch = Vec::new();
    let flush = |batch: &mut Vec<Event>, deliver: &mut Deliver| {
        if !batch.is_empty() {
            deliver(Out::Events(std::mem::take(batch)));
        }
    };
    while let Ok(item) = events.recv() {
        let mut next = Some(item);
        let deadline = last.map_or_else(Instant::now, |last| last + every);
        loop {
            match next.take() {
                Some(Item::Event(event)) => batch.push(event),
                Some(Item::Ended { status, stderr }) => {
                    flush(&mut batch, &mut deliver);
                    return deliver(Out::Ended { status, stderr });
                }
                None => {}
            }
            let now = Instant::now();
            if batch.len() >= MAX_BATCH || now >= deadline {
                break;
            }
            match events.recv_timeout(deadline.saturating_duration_since(now)) {
                Ok(item) => next = Some(item),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    flush(&mut batch, &mut deliver);
                    return deliver(Out::Ended {
                        status: None,
                        stderr: String::new(),
                    });
                }
            }
        }
        last = Some(Instant::now());
        flush(&mut batch, &mut deliver);
    }
    flush(&mut batch, &mut deliver);
    deliver(Out::Ended {
        status: None,
        stderr: String::new(),
    });
}

/// Keeps the last [`MAX_ERR`] bytes of `stderr` in `kept` until it ends.
fn keep_end(mut stderr: impl std::io::Read, kept: &Mutex<Vec<u8>>) {
    let mut chunk = [0_u8; 4096];
    while let Ok(read) = stderr.read(&mut chunk) {
        if read == 0 {
            return;
        }
        let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.extend_from_slice(&chunk[..read]);
        let over = kept.len().saturating_sub(MAX_ERR);
        kept.drain(..over);
    }
}

#[cfg(test)]
mod tests;
