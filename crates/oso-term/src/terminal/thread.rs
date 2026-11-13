//! A terminal's thread (ADR 0120, Q4): it opens the PTY, starts the child in it, and then
//! reads at most 64 KiB at a time, parses at most 1 MiB before it looks at the UI's requests
//! again, writes what is queued, judges each effect, and sends a snapshot when the last one
//! was taken. When the PTY ends it reaps the child and sends the end.
//!
//! Must not depend on: the UI thread's state; it shares only the channels and `Shared`.

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
        let mut session = Session::open(start, size, wakeup, requests, snapshots, shared, wake)?;
        let end = session.run();
        last = session.finish();
        Ok(match end {
            End::Closed => session.reap().map_or(End::Closed, End::Exited),
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

/// A running terminal, on its thread.
struct Session<'a> {
    master: Master,
    child: Option<Child>,
    engine: Ghostty,
    wakeup: &'a Wakeup,
    requests: &'a mpsc::Receiver<Request>,
    snapshots: &'a mpsc::Sender<Snapshot>,
    shared: &'a Shared,
    wake: &'a dyn Fn(),
    /// Replies to the program, written before its input.
    replies: Vec<u8>,
    input: Vec<u8>,
    /// Bytes of `input` already written.
    written: usize,
    read: Vec<u8>,
    effects: Vec<Effect>,
    /// The snapshot being filled until the UI took the last one.
    next: Snapshot,
    spare: Vec<Row>,
    /// Whether `next` holds anything worth sending.
    changed: bool,
    echo_off: bool,
    /// The requests ended: the handle is gone.
    detached: bool,
}

impl<'a> Session<'a> {
    fn open(
        start: Start,
        size: Size,
        wakeup: &'a Wakeup,
        requests: &'a mpsc::Receiver<Request>,
        snapshots: &'a mpsc::Sender<Snapshot>,
        shared: &'a Shared,
        wake: &'a dyn Fn(),
    ) -> io::Result<Session<'a>> {
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
        let scrollback = start.scrollback.min(limits::SCROLLBACK_MAX);
        let mut engine = Ghostty::new(size, scrollback)
            .ok_or_else(|| io::Error::other("the terminal engine could not start"))?;
        engine.set_palette(&start.palette);
        Ok(Session {
            master,
            child: Some(child),
            engine,
            wakeup,
            requests,
            snapshots,
            shared,
            wake,
            replies: Vec::new(),
            input: Vec::new(),
            written: 0,
            read: vec![0; limits::READ_CHUNK],
            effects: Vec::new(),
            next: Snapshot::new(size),
            spare: Vec::new(),
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
            let (ended, more) = match self.read() {
                Ok(read) => read,
                Err(error) => return End::Failed(error),
            };
            self.judge();
            self.publish();
            if ended {
                return End::Closed;
            }
            let timeout = more.then_some(Duration::ZERO);
            let writable = !self.replies.is_empty() || self.written < self.input.len();
            if let Err(error) = self.master.wait(self.wakeup, writable, timeout) {
                return End::Failed(error);
            }
        }
    }

    fn serve_requests(&mut self) {
        loop {
            match self.requests.try_recv() {
                Ok(Request::Input(bytes)) => self.input.extend_from_slice(&bytes),
                Ok(Request::Resize(size)) => {
                    let size = limits::clamp(size);
                    let _ = self.master.resize(size);
                    self.engine.resize(size);
                    self.changed = true;
                }
                Ok(Request::Palette(palette)) => {
                    self.engine.set_palette(&palette);
                    self.changed = true;
                }
                Ok(Request::Recycle(mut snapshot)) => {
                    snapshot.clear(&mut self.spare);
                    for row in &mut self.spare {
                        row.cells.clear();
                        row.graphemes.clear();
                    }
                }
                Ok(Request::HangUp) => {
                    let _ = self.master.hang_up();
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.detached = true;
                    return;
                }
            }
        }
    }

    /// Writes replies, then input, until the PTY would block.
    fn write(&mut self) -> io::Result<()> {
        while !self.replies.is_empty() {
            match self.master.write(&self.replies) {
                Ok(n) => drop(self.replies.drain(..n)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        while self.written < self.input.len() {
            match self.master.write(&self.input[self.written..]) {
                Ok(n) => {
                    self.written += n;
                    self.shared.queued.fetch_sub(n, Ordering::AcqRel);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if self.written == self.input.len() {
            self.input.clear();
            self.written = 0;
        }
        Ok(())
    }

    /// Reads and parses until the PTY would block, it ended, or a slice is parsed. Returns
    /// whether it ended, and whether more may wait.
    fn read(&mut self) -> io::Result<(bool, bool)> {
        let mut parsed = 0;
        while parsed < limits::PARSE_SLICE {
            match self.master.read(&mut self.read) {
                Ok(0) => return Ok((true, false)),
                Ok(n) => {
                    self.engine.feed(&self.read[..n]);
                    parsed += n;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok((false, false)),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok((false, true))
    }

    /// Judges the effects the output raised: replies queue, events wait for the snapshot.
    fn judge(&mut self) {
        self.engine.take_effects(&mut self.effects);
        for effect in self.effects.drain(..) {
            match policy::judge(effect) {
                Verdict::Reply(bytes) => {
                    if self.replies.len() + bytes.len() <= limits::REPLIES_PENDING_MAX {
                        self.replies.extend_from_slice(&bytes);
                    }
                }
                Verdict::Event(event) => {
                    self.changed = true;
                    let bell_again = matches!(event, Event::Bell)
                        && matches!(self.next.events.last(), Some(Event::Bell));
                    if bell_again {
                        continue;
                    }
                    if self.next.events.len() < limits::EVENTS_MAX {
                        self.next.events.push(event);
                    } else {
                        self.next.dropped_events = self.next.dropped_events.saturating_add(1);
                    }
                }
                Verdict::Drop => {}
            }
        }
    }

    /// Sends a snapshot if the UI took the last one and something changed since.
    fn publish(&mut self) {
        // A failed check reads as echo off, as on systems with no echo flag.
        let echo_off = !self.master.echoes().unwrap_or(false);
        self.changed |= echo_off != self.echo_off;
        self.echo_off = echo_off;
        if self.shared.in_flight.load(Ordering::Acquire) {
            return;
        }
        self.changed |= self.engine.snapshot(&mut self.next, &mut self.spare);
        if !std::mem::take(&mut self.changed) {
            return;
        }
        self.next.echo_off = echo_off;
        let size = self.next.size;
        let snapshot = std::mem::replace(&mut self.next, Snapshot::new(size));
        self.shared.in_flight.store(true, Ordering::Release);
        if self.snapshots.send(snapshot).is_ok() {
            (self.wake)();
        }
    }

    /// The last snapshot: what the output did before it ended.
    fn finish(&mut self) -> Snapshot {
        self.judge();
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

impl Drop for Session<'_> {
    fn drop(&mut self) {
        let _ = self.reap();
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
