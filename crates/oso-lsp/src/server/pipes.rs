//! A running server's three pipe threads (ADR 0138, Threads): the writer, which owns its
//! standard input and writes what the session thread queued, at most
//! [`WRITER_QUEUE_BYTES_MAX`] of it; the reader, which owns its standard output and hands each
//! frame to the session thread unparsed, holding at most [`READER_QUEUE_BYTES_MAX`] there
//! before it waits; and the error reader, which keeps the last [`STDERR_KEPT_BYTES`] of its
//! standard error as lines cut to [`SHOWN_BYTES_MAX`], for the session thread to take.
//!
//! Must not depend on: what messages mean, or the outbox; these threads move bytes, and the
//! session thread reads them.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::mem;
use std::process::{ChildStderr, ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use crate::framing::{self, Frame, FrameError, Reader};
use crate::limits::{
    READ_PIECE, READER_QUEUE_BYTES_MAX, SHOWN_BYTES_MAX, STDERR_KEPT_BYTES, WRITER_QUEUE_BYTES_MAX,
};

use super::Event;

/// The writer's queue was past its bound: the server is stopped, as sync cannot drop a
/// message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct QueueFull;

/// The session thread's end of the writer.
#[derive(Debug)]
pub(super) struct Writer {
    queue: Sender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
}

impl Writer {
    /// How many bytes wait to be written.
    pub(super) fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    /// Queues `body`, framed.
    pub(super) fn send(&self, body: &[u8]) -> Result<(), QueueFull> {
        let frame = framing::encode(body);
        let len = frame.len();
        if self.queued.fetch_add(len, Ordering::SeqCst) + len > WRITER_QUEUE_BYTES_MAX {
            self.queued.fetch_sub(len, Ordering::SeqCst);
            return Err(QueueFull);
        }
        // A writer that ended has said why on the events.
        let _ = self.queue.send(frame);
        Ok(())
    }
}

/// How many bytes the reader handed over that the session thread has not taken, and whether
/// the run has ended, so that a waiting reader stops.
#[derive(Debug, Default)]
pub(super) struct Credit {
    held: Mutex<(usize, bool)>,
    taken: Condvar,
}

impl Credit {
    /// Waits until `len` more bytes fit, or one message does when none waits. `false` once the
    /// run has ended.
    fn acquire(&self, len: usize) -> bool {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        while !held.1 && held.0 > 0 && held.0 + len > READER_QUEUE_BYTES_MAX {
            held = self
                .taken
                .wait(held)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if held.1 {
            return false;
        }
        held.0 += len;
        true
    }

    /// The session thread took a frame of `len` bytes.
    pub(super) fn release(&self, len: usize) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.0 = held.0.saturating_sub(len);
        self.taken.notify_all();
    }

    /// The run ended: a reader waiting for room stops.
    pub(super) fn close(&self) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.taken.notify_all();
    }
}

/// What a frame handed to the session thread takes beyond its bytes: its event, its vector
/// and its place in the channel.
pub(super) const FRAME_COST: usize = 128;

/// What a kept line takes beyond its bytes, so that empty lines count against the bound.
const LINE_COST: usize = std::mem::size_of::<String>();

/// The lines of the standard error not yet taken, the last [`STDERR_KEPT_BYTES`] of them.
#[derive(Debug, Default)]
pub(super) struct Stderr(Mutex<Kept>);

#[derive(Debug, Default)]
struct Kept {
    lines: VecDeque<String>,
    bytes: usize,
    dropped: usize,
}

impl Stderr {
    /// The lines kept since the last call, and how many went for the bound.
    pub(super) fn take(&self) -> (Vec<String>, usize) {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        kept.bytes = 0;
        (kept.lines.drain(..).collect(), mem::take(&mut kept.dropped))
    }

    #[cfg(test)]
    pub(super) fn push_for_tests(&self, line: &[u8]) {
        self.push(line);
    }

    fn push(&self, line: &[u8]) {
        let line = String::from_utf8_lossy(line);
        let line = &line[..line.floor_char_boundary(SHOWN_BYTES_MAX)];
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        // What a line takes, an empty one too.
        kept.bytes += line.len() + LINE_COST;
        kept.lines.push_back(line.to_owned());
        while kept.bytes > STDERR_KEPT_BYTES {
            let Some(oldest) = kept.lines.pop_front() else {
                break;
            };
            kept.bytes -= oldest.len() + LINE_COST;
            kept.dropped += 1;
        }
    }
}

/// Starts the three threads of run `run` of server `number`. Each says on `events` how it
/// ended.
pub(super) fn start(
    number: usize,
    run: u64,
    pipes: (ChildStdin, ChildStdout, ChildStderr),
    events: &Sender<Event>,
) -> io::Result<(Writer, Arc<Credit>, Arc<Stderr>)> {
    let (stdin, stdout, stderr) = pipes;
    let (queue, frames) = mpsc::channel::<Vec<u8>>();
    let queued = Arc::new(AtomicUsize::new(0));
    let credit = Arc::new(Credit::default());
    let kept = Arc::new(Stderr::default());
    {
        let queued = Arc::clone(&queued);
        let events = events.clone();
        thread::Builder::new()
            .name(format!("oso-lsp-{number}-w"))
            .spawn(move || write(stdin, &frames, &queued, run, &events))?;
    }
    {
        let credit = Arc::clone(&credit);
        let events = events.clone();
        thread::Builder::new()
            .name(format!("oso-lsp-{number}-r"))
            .spawn(move || read(stdout, &credit, run, &events))?;
    }
    {
        let kept = Arc::clone(&kept);
        thread::Builder::new()
            .name(format!("oso-lsp-{number}-e"))
            .spawn(move || read_errors(stderr, &kept))?;
    }
    Ok((Writer { queue, queued }, credit, kept))
}

/// Writes each frame queued until the queue closes, then closes the standard input.
fn write(
    mut stdin: ChildStdin,
    frames: &mpsc::Receiver<Vec<u8>>,
    queued: &AtomicUsize,
    run: u64,
    events: &Sender<Event>,
) {
    for frame in frames {
        let written = stdin.write_all(&frame).and_then(|()| stdin.flush());
        queued.fetch_sub(frame.len(), Ordering::SeqCst);
        if let Err(error) = written {
            let _ = events.send(Event::WriteFailed(run, error.kind()));
            return;
        }
    }
}

/// Hands each frame to the session thread until the output ends or breaks.
fn read(stdout: ChildStdout, credit: &Credit, run: u64, events: &Sender<Event>) {
    let mut reader = Reader::new(stdout);
    loop {
        let next = reader.next_frame();
        let frame = match next {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let _ = events.send(Event::ReadEnd(run, None));
                return;
            }
            Err(error) => {
                let _ = events.send(Event::ReadEnd(run, Some(error)));
                return;
            }
        };
        // What a frame takes beyond its bytes counts too, so that small ones cannot queue
        // without bound.
        let len = FRAME_COST
            + match &frame {
                Frame::Message(bytes) => bytes.len(),
                Frame::Dropped { .. } => 0,
            };
        if !credit.acquire(len) || events.send(Event::Frame(run, frame)).is_err() {
            return;
        }
    }
}

/// Keeps the standard error's lines until it ends.
fn read_errors(mut stderr: ChildStderr, kept: &Stderr) {
    let mut piece = vec![0; READ_PIECE];
    let mut line: Vec<u8> = Vec::new();
    loop {
        let read = match stderr.read(&mut piece) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &piece[..read] {
            if byte == b'\n' {
                kept.push(&line);
                line.clear();
            } else if line.len() < SHOWN_BYTES_MAX + 4 {
                // Past the cut, the rest of the line is not kept: a line is cut anyway.
                line.push(byte);
            }
        }
    }
    if !line.is_empty() {
        kept.push(&line);
    }
}

/// Why a run's output ended, as `*LSP*` says it.
pub(super) fn ended(error: Option<&FrameError>) -> String {
    match error {
        None => "closed its output".into(),
        Some(FrameError::Broken(broken)) => format!("sent a broken header: {broken}"),
        Some(FrameError::Truncated) => "closed its output inside a message".into(),
        Some(FrameError::Io(kind)) => format!("its output could not be read: {kind}"),
    }
}
