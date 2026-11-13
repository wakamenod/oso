//! A terminal's thread (ADR 0120, Q4): it makes the engine, opens the PTY and starts the child
//! in it, and then reads at most 64 KiB at a time, parses for at most 4 ms (and at most 1 MiB)
//! before it looks at the UI's requests again, writes what is queued, judges each effect, and
//! sends a snapshot when the last one was taken. When the PTY ends it reaps the child and sends
//! the end.
//!
//! Must not depend on: the UI thread's state; it shares only the channels and `Shared`.

use std::collections::VecDeque;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Child, ExitStatus};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

use oso_platform::process::pty::{self, Master, Size, Wakeup};
use oso_platform::process::{self as spawning, Sink, Source, Spawn};

use super::{Request, Shared, Start};
use crate::engine::ghostty::Ghostty;
use crate::engine::{Effect, Engine};
use crate::event::{End, Event};
use crate::limits;
use crate::policy::{self, Verdict};
use crate::snapshot::{Row, Snapshot};

/// How often, and for how long, a child is looked for after the PTY ended.
const REAP_EVERY: Duration = Duration::from_millis(50);
const REAP_FOR: Duration = Duration::from_secs(2);

/// Runs a terminal to its end, and sends the end as the last snapshot's event.
pub(super) fn run(
    start: Start,
    wakeup: &Wakeup,
    requests: &mpsc::Receiver<Request>,
    snapshots: &mpsc::Sender<Snapshot>,
    shared: &Shared,
    wake: &dyn Fn(),
) {
    let size = limits::clamp(start.size);
    let mut last = Snapshot::new(size);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let channels = Channels {
            wakeup,
            requests,
            snapshots,
            shared,
            wake,
        };
        let mut running = Running::open(start, size, channels)?;
        let end = running.run();
        last = running.finish();
        if running.detached {
            // No one waits for the end: the child is left to a reaper at once.
            if let Some(child) = running.child.take() {
                reap_later(child);
            }
        }
        Ok(match end {
            End::Closed => running.reap().map_or(End::Closed, End::Exited),
            other => other,
        })
    }));
    let end = match outcome {
        Ok(Ok(end)) => end,
        Ok(Err(error)) => End::Failed(error),
        Err(_) => End::Failed(io::Error::other("the terminal's thread failed")),
    };
    log::debug!("a terminal ended: {end:?}");
    last.events.push(Event::Ended(end));
    // The end goes even with a snapshot in flight; nothing follows it.
    if snapshots.send(last).is_ok() {
        wake();
    }
}

/// Judges the effects the output raised: replies queue, while 64 KiB wait at most; events go
/// into `next`. A title, a directory, a progress or a clipboard write replaces the one of its
/// kind already there, and is never dropped, so a snapshot holds the latest of each; the rest,
/// at most 256, are counted past that, a bell right after a bell joined to it. Returns whether
/// an event came. The terminal's thread and the fuzz target both judge through this.
pub(super) fn judge(effects: &mut Vec<Effect>, replies: &mut Vec<u8>, next: &mut Snapshot) -> bool {
    let mut evented = false;
    for effect in effects.drain(..) {
        match policy::judge(effect) {
            Verdict::Reply(bytes) => {
                if replies.len() + bytes.len() <= limits::REPLIES_PENDING_MAX {
                    replies.extend_from_slice(&bytes);
                }
            }
            Verdict::Event(event) => {
                evented = true;
                keep(event, next);
            }
            Verdict::Drop => {}
        }
    }
    evented
}

fn keep(event: Event, next: &mut Snapshot) {
    let kind = std::mem::discriminant(&event);
    let latest = matches!(
        event,
        Event::Title(_) | Event::Directory(_) | Event::Progress(_) | Event::ClipboardWrite(_)
    );
    if latest {
        if let Some(at) = next
            .events
            .iter_mut()
            .find(|e| std::mem::discriminant(*e) == kind)
        {
            *at = event;
        } else {
            next.events.push(event);
        }
        return;
    }
    if matches!(event, Event::Bell) && matches!(next.events.last(), Some(Event::Bell)) {
        return;
    }
    if next.events.len() < limits::EVENTS_MAX {
        next.events.push(event);
    } else {
        next.dropped_events = next.dropped_events.saturating_add(1);
    }
}

/// What the thread shares with the handle.
struct Channels<'a> {
    wakeup: &'a Wakeup,
    requests: &'a mpsc::Receiver<Request>,
    snapshots: &'a mpsc::Sender<Snapshot>,
    shared: &'a Shared,
    wake: &'a dyn Fn(),
}

/// How a read slice ended.
enum Slice {
    /// The PTY would block: nothing more to read now.
    Drained,
    /// The slice's time or bytes ran out, or the UI wants something: more may wait.
    Cut,
    /// The PTY ended.
    Ended,
}

/// A running terminal, on its thread.
struct Running<'a> {
    master: Master,
    child: Option<Child>,
    engine: Ghostty,
    channels: Channels<'a>,
    /// Replies to the program, written before its input.
    replies: Vec<u8>,
    /// The input queued, in the chunks it came in, and what of the first is written.
    input: VecDeque<Vec<u8>>,
    written: usize,
    /// A resize waiting to be applied, the last of those asked.
    resize: Option<Size>,
    read: Vec<u8>,
    effects: Vec<Effect>,
    /// The snapshot being filled until the UI took the last one.
    next: Snapshot,
    spare: Vec<Row>,
    /// A snapshot the UI gave back, reused as the next `next`.
    recycled: Option<Snapshot>,
    /// Whether `next` holds anything worth sending.
    changed: bool,
    echo_off: bool,
    /// The requests ended: the handle is gone.
    detached: bool,
}

impl<'a> Running<'a> {
    fn open(start: Start, size: Size, channels: Channels<'a>) -> io::Result<Running<'a>> {
        // The engine first: once a child runs, an error must not leave it unreaped.
        let scrollback = start.scrollback.min(limits::SCROLLBACK_MAX);
        let mut engine = Ghostty::new(size, scrollback, policy::XTVERSION)
            .ok_or_else(|| io::Error::other("the terminal engine could not start"))?;
        engine.set_palette(&start.palette);
        let (master, slave) = pty::open(size)?;
        let inherit: Vec<&str> = start.inherit.iter().map(String::as_str).collect();
        let set: Vec<(&str, &std::ffi::OsStr)> = start
            .set
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_os_str()))
            .collect();
        let child = spawning::spawn(&Spawn {
            executable: &start.executable,
            args: &start.args,
            cwd: start.cwd.as_deref(),
            project: start.project.as_deref(),
            inherit: &inherit,
            set: &set,
            stdin: Source::Closed,
            stdout: Sink::Inherit,
            stderr: Sink::Inherit,
            own_process_group: false,
            terminal: Some(&slave),
        })?;
        // The master sees the terminal's end only once no slave is open here.
        drop(slave);
        Ok(Running {
            master,
            child: Some(child),
            engine,
            channels,
            replies: Vec::new(),
            input: VecDeque::new(),
            written: 0,
            resize: None,
            read: vec![0; limits::READ_CHUNK],
            effects: Vec::new(),
            next: Snapshot::new(size),
            spare: Vec::new(),
            recycled: None,
            changed: true,
            echo_off: false,
            detached: false,
        })
    }

    /// Serves the terminal until its PTY ends or its handle is dropped.
    fn run(&mut self) -> End {
        loop {
            self.serve_requests();
            if self.detached {
                let _ = self.master.hang_up();
                return End::Closed;
            }
            if let Err(error) = self.write() {
                return End::Failed(error);
            }
            let slice = match self.read() {
                Ok(slice) => slice,
                Err(error) => return End::Failed(error),
            };
            self.engine.take_effects(&mut self.effects);
            self.changed |= judge(&mut self.effects, &mut self.replies, &mut self.next);
            self.publish();
            let timeout = match slice {
                Slice::Ended => return End::Closed,
                Slice::Cut => Some(Duration::ZERO),
                Slice::Drained => None,
            };
            let writable = !self.replies.is_empty() || !self.input.is_empty();
            let channels = &self.channels;
            if let Err(error) = self.master.wait(channels.wakeup, writable, timeout) {
                return End::Failed(error);
            }
        }
    }

    fn serve_requests(&mut self) {
        self.channels
            .shared
            .requested
            .store(false, Ordering::Release);
        loop {
            match self.channels.requests.try_recv() {
                Ok(Request::Input(bytes)) => self.input.push_back(bytes),
                Ok(Request::Resize(size)) => self.resize = Some(limits::clamp(size)),
                Ok(Request::Palette(palette)) => {
                    self.engine.set_palette(&palette);
                    self.changed = true;
                }
                Ok(Request::Recycle(mut snapshot)) => {
                    snapshot.clear(&mut self.spare);
                    self.recycled = Some(snapshot);
                }
                Ok(Request::HangUp) => {
                    let _ = self.master.hang_up();
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.detached = true;
                    break;
                }
            }
        }
        // Resizes coalesce: a drag asks many, and each makes the program redraw.
        if let Some(size) = self.resize.take()
            && size != self.next.size
        {
            let _ = self.master.resize(size);
            self.engine.resize(size);
            self.next.size = size;
            self.changed = true;
        }
    }

    /// Writes until the PTY would block. Replies go between input chunks, whole, never inside
    /// one: a reply inside a paste would split its bracket and let its lines run.
    fn write(&mut self) -> io::Result<()> {
        loop {
            if self.written == 0 && !self.replies.is_empty() {
                let Some(n) = written(self.master.write(&self.replies))? else {
                    return Ok(());
                };
                drop(self.replies.drain(..n));
                continue;
            }
            let Some(chunk) = self.input.front() else {
                return Ok(());
            };
            let Some(n) = written(self.master.write(&chunk[self.written..]))? else {
                return Ok(());
            };
            self.written += n;
            self.channels.shared.queued.fetch_sub(n, Ordering::AcqRel);
            if self.written == chunk.len() {
                self.input.pop_front();
                self.written = 0;
            }
        }
    }

    /// Reads and parses until the PTY would block or ends, the slice's time or bytes run out,
    /// or the UI asked for something or took the last snapshot.
    fn read(&mut self) -> io::Result<Slice> {
        let started = Instant::now();
        let mut parsed = 0;
        loop {
            match self.master.read(&mut self.read) {
                Ok(0) => return Ok(Slice::Ended),
                Ok(n) => {
                    self.engine.feed(&self.read[..n]);
                    parsed += n;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(Slice::Drained),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            let shared = self.channels.shared;
            let wanted = shared.requested.load(Ordering::Acquire)
                || !shared.in_flight.load(Ordering::Acquire);
            if wanted || parsed >= limits::PARSE_SLICE || started.elapsed() >= limits::PARSE_TIME {
                return Ok(Slice::Cut);
            }
        }
    }

    /// Sends a snapshot if the UI took the last one and something changed since.
    fn publish(&mut self) {
        let shared = self.channels.shared;
        if shared.in_flight.load(Ordering::Acquire) {
            return;
        }
        // A failed check reads as echo off, as on systems with no echo flag.
        let echo_off = !self.master.echoes().unwrap_or(false);
        self.changed |= echo_off != self.echo_off;
        self.echo_off = echo_off;
        self.changed |= self.engine.snapshot(&mut self.next, &mut self.spare);
        if !std::mem::take(&mut self.changed) {
            return;
        }
        self.next.echo_off = echo_off;
        let size = self.next.size;
        let fresh = self.recycled.take().unwrap_or_else(|| Snapshot::new(size));
        let snapshot = std::mem::replace(&mut self.next, fresh);
        self.next.size = size;
        shared.in_flight.store(true, Ordering::Release);
        if self.channels.snapshots.send(snapshot).is_ok() {
            (self.channels.wake)();
        }
    }

    /// The last snapshot: what the output did before it ended.
    fn finish(&mut self) -> Snapshot {
        self.engine.take_effects(&mut self.effects);
        judge(&mut self.effects, &mut self.replies, &mut self.next);
        self.engine.snapshot(&mut self.next, &mut self.spare);
        self.next.echo_off = self.echo_off;
        let size = self.next.size;
        std::mem::replace(&mut self.next, Snapshot::new(size))
    }

    /// The child's exit status, if it exits within `REAP_FOR`. One that outlives its
    /// terminal is left to a thread of its own, so that no zombie stays.
    fn reap(&mut self) -> Option<ExitStatus> {
        let mut child = self.child.take()?;
        let since = Instant::now();
        while since.elapsed() < REAP_FOR {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => std::thread::sleep(REAP_EVERY),
                Err(_) => return None,
            }
        }
        reap_later(child);
        None
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let _ = self.reap();
    }
}

/// How many bytes a write took: `None` when the PTY would block, or took none.
fn written(result: io::Result<usize>) -> io::Result<Option<usize>> {
    match result {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n)),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(Some(0)),
        Err(e) => Err(e),
    }
}

fn reap_later(mut child: Child) {
    let spawned = std::thread::Builder::new()
        .name("oso-terminal-reaper".into())
        .spawn(move || child.wait());
    if spawned.is_err() {
        log::warn!("a terminal's child could not be reaped");
    }
}
