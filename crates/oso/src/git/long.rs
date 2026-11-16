//! Running git with no time limit (ADR 0127, step 1b): a commit with its hooks, which may run a
//! whole test suite, and a write to the index, which a kill could leave locked. Each run is the
//! person's act, and they stop it: `C-g` sends `SIGTERM`, then `SIGKILL` after [`GRACE`].
//! A caller may also set no stop and keep no [`Ender`]: a worktree's removal, which a stop
//! would leave half removed (ADR 0121, "Worktree removal, as fixed"). Such a run is ended only
//! by a [`Long`] dropped before git ends, as an error reading it drops it.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on the threads that
//! run git, and waits for git, but for [`Ender::end`], which the UI thread calls and which
//! waits for nothing.
//!
//! - git starts in a process group of its own, so that stopping it stops a hook and what the
//!   hook started too; on Windows git alone (ADR 0127, as built, Q6). A stopped run is reaped
//!   only after its group gets `SIGKILL`, sent once git has ended but while it is not yet waited
//!   for ([`Child::exited`]), so that its ID still holds the group's: a hook that ignores
//!   `SIGTERM`, and what it started, go with git. Quitting sends `SIGTERM`, then `SIGKILL` at
//!   once (the author's choice): oso does not wait as it goes.
//! - Its standard output and error are read as one, in pieces, on threads of their own,
//!   `oso-git-read`, as a terminal would show them; the caller bounds what it keeps. At most
//!   [`PIECES`] pieces wait, so a hook that writes faster than they are taken waits on its
//!   pipe. Once git has gone, what the pipes still hold is read for at most [`DRAIN`], and at
//!   most [`MAX_TAIL`] bytes of it kept: a process the hook left may hold them open.
//! - The child is shared with an [`Ender`], so that quitting oso ends the group at once from
//!   the UI thread, which sends a signal and waits for nothing.
//! - A [`Long`] dropped while git runs ends it as a stop would, on a thread of its own,
//!   `oso-git-reap`, which waits for it.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Child, Signal, Sink, Source};

use super::run::{Git, GitError, MAX_STDERR, Output, write_away};

/// How long a stopped run has between `SIGTERM` and `SIGKILL`.
pub(crate) const GRACE: Duration = Duration::from_secs(5);
/// How long a run that has exited is given, in all, to have its last output read.
const DRAIN: Duration = Duration::from_millis(200);
/// The most bytes of that last output kept: its end.
const MAX_TAIL: usize = 4 * 1024 * 1024;
/// The most pieces of output that wait to be taken.
const PIECES: usize = 64;
/// How often a run whose output has ended, or that is stopping, is looked at.
const POLL: Duration = Duration::from_millis(20);
/// The most bytes one piece of output holds.
const PIECE: usize = 8192;

/// What one look at a long run found.
#[derive(Debug)]
pub(crate) enum Next {
    /// What it wrote since, on either stream.
    Out(Vec<u8>),
    /// Nothing yet.
    Quiet,
    /// It ended: how, once every piece was given.
    Ended(ExitStatus),
}

/// What a reading thread sends.
enum Piece {
    Out(Vec<u8>),
    Closed,
}

/// A run of git with no time limit.
pub(crate) struct Long {
    child: Arc<Mutex<Child>>,
    pieces: Receiver<Piece>,
    /// The streams not yet at their end.
    open: u8,
    /// When it was asked to stop, and whether `SIGKILL` followed.
    stopping: Option<Instant>,
    killed: bool,
    /// How it ended, once waited for; the output read after is given first.
    exited: Option<ExitStatus>,
    tail: Vec<u8>,
}

/// What ends a long run's group from elsewhere, as quitting oso does.
#[derive(Clone)]
pub(crate) struct Ender(Arc<Mutex<Child>>);

impl Ender {
    /// Sends the run's group `SIGTERM`, then `SIGKILL` at once, so that a hook that ignores the
    /// first goes too; waits for nothing.
    pub(crate) fn end(&self) {
        let mut child = locked(&self.0);
        let _ = child.signal_group(Signal::Terminate);
        let _ = child.signal_group(Signal::Kill);
    }
}

impl Git {
    /// Starts git with `args` after oso's own, `set` over the environment and `input` on its
    /// standard input if there is one, in a process group of its own, with no time limit. What
    /// it writes comes through [`Long::next`].
    pub(crate) fn start_long(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        input: Option<Vec<u8>>,
    ) -> Result<Long, GitError> {
        let stdin = if input.is_some() {
            Source::Piped
        } else {
            Source::Closed
        };
        let child = self.spawn_with(args, set, stdin, Sink::Piped, true)?;
        Long::adopt(child, input)
    }
}

impl Long {
    /// The long run of `child`, started in a process group of its own with its output piped, and
    /// its input piped if `input` is: `gh`'s too (ADR 0127, step 5a).
    pub(crate) fn adopt(mut child: Child, input: Option<Vec<u8>>) -> Result<Long, GitError> {
        let streams = (child.stdout.take(), child.stderr.take());
        let (Some(stdout), Some(stderr)) = streams else {
            end_now(&mut child);
            return Err(GitError::Unexpected("no output streams"));
        };
        if let Some(input) = input
            && let Err(e) = write_away(&mut child, input)
        {
            end_now(&mut child);
            return Err(e);
        }
        let (sent, pieces) = mpsc::sync_channel(PIECES);
        let started = reader(stdout, sent.clone()).and_then(|()| reader(stderr, sent));
        if let Err(e) = started {
            end_now(&mut child);
            return Err(GitError::Io(e));
        }
        Ok(Long {
            child: Arc::new(Mutex::new(child)),
            pieces,
            open: 2,
            stopping: None,
            killed: false,
            exited: None,
            tail: Vec::new(),
        })
    }
}

impl Git {
    /// Runs git as [`Git::start_long`] does, until it ends or `stop` is set, and keeps the
    /// start of what it wrote, at most [`MAX_STDERR`] bytes, as standard error: for a write to
    /// the index, whose failure says why there.
    pub(crate) fn run_long(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        input: Option<Vec<u8>>,
        stop: &AtomicBool,
    ) -> Result<Output, GitError> {
        let mut long = self.start_long(args, set, input)?;
        let mut said = Vec::new();
        loop {
            if stop.load(Ordering::SeqCst) {
                long.stop();
            }
            match long.next(POLL)? {
                Next::Out(piece) => {
                    let room = MAX_STDERR.saturating_sub(said.len()).min(piece.len());
                    said.extend_from_slice(&piece[..room]);
                }
                Next::Quiet => {}
                Next::Ended(_) if long.stopped() => return Err(GitError::Stopped),
                Next::Ended(status) => {
                    return Ok(Output {
                        stdout: Vec::new(),
                        cut: false,
                        status,
                        stderr: String::from_utf8_lossy(&said).into_owned(),
                    });
                }
            }
        }
    }
}

impl Long {
    /// Waits up to `wait` for what the run writes next, or its end.
    pub(crate) fn next(&mut self, wait: Duration) -> Result<Next, GitError> {
        if let Some(status) = self.exited {
            if !self.tail.is_empty() {
                return Ok(Next::Out(std::mem::take(&mut self.tail)));
            }
            return Ok(Next::Ended(status));
        }
        if let Some(since) = self.stopping
            && !self.killed
            && since.elapsed() >= GRACE
        {
            self.killed = true;
            let _ = locked(&self.child).signal_group(Signal::Kill);
        }
        let mut got = None;
        if self.open > 0 {
            match self.pieces.recv_timeout(wait) {
                Ok(Piece::Out(piece)) => got = Some(piece),
                Ok(Piece::Closed) => self.open -= 1,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.open = 0,
            }
        } else {
            thread::sleep(wait.min(POLL));
        }
        // Looked at after each piece too, so that what goes on writing after git has gone holds
        // up its end no longer than the drain.
        let status = self.reap_ended().map_err(GitError::Io)?;
        let Some(status) = status else {
            return Ok(got.map_or(Next::Quiet, Next::Out));
        };
        self.tail.extend(got.into_iter().flatten());
        // What the readers still hold, until they end or the time is up: a process git started
        // may keep the pipes open after git has gone, and write on.
        let deadline = Instant::now() + DRAIN;
        while self.open > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.pieces.recv_timeout(left) {
                Ok(Piece::Out(piece)) => {
                    self.tail.extend_from_slice(&piece);
                    if self.tail.len() > 2 * MAX_TAIL {
                        let cut = self.tail.len() - MAX_TAIL;
                        self.tail.drain(..cut);
                    }
                }
                Ok(Piece::Closed) => self.open -= 1,
                Err(_) => break,
            }
        }
        if self.tail.len() > MAX_TAIL {
            let cut = self.tail.len() - MAX_TAIL;
            self.tail.drain(..cut);
        }
        self.exited = Some(status);
        self.next(Duration::ZERO)
    }

    /// How the run ended, reaping it, or `None` while git runs. A stopped run's group gets
    /// `SIGKILL` first, while git's ID still holds the group's, so that what ignored `SIGTERM`
    /// goes too.
    fn reap_ended(&mut self) -> io::Result<Option<ExitStatus>> {
        let mut child = locked(&self.child);
        if self.stopping.is_some() && child.exited()? {
            // Nothing left in the group is no error: ESRCH, or EPERM on macOS.
            let _ = child.signal_group(Signal::Kill);
            self.killed = true;
        }
        child.try_wait()
    }

    /// Asks the run to stop: `SIGTERM` to its group now, and `SIGKILL` once git has ended or
    /// [`GRACE`] has passed, as [`Long::next`] is called.
    pub(crate) fn stop(&mut self) {
        if self.stopping.is_none() && self.exited.is_none() {
            self.stopping = Some(Instant::now());
            let _ = locked(&self.child).signal_group(Signal::Terminate);
        }
    }

    /// Whether it was asked to stop.
    pub(crate) fn stopped(&self) -> bool {
        self.stopping.is_some()
    }

    /// What ends the run's group from elsewhere.
    pub(crate) fn ender(&self) -> Ender {
        Ender(Arc::clone(&self.child))
    }
}

impl Drop for Long {
    fn drop(&mut self) {
        if self.exited.is_some() {
            return;
        }
        self.stop();
        let child = Arc::clone(&self.child);
        let reaped = thread::Builder::new()
            .name("oso-git-reap".into())
            .spawn(move || reap(&child));
        if reaped.is_err() {
            let _ = locked(&self.child).signal_group(Signal::Kill);
        }
    }
}

/// Waits for a run that was asked to stop, killing its group once git has ended, before it is
/// reaped, or after [`GRACE`].
fn reap(child: &Mutex<Child>) {
    let deadline = Instant::now() + GRACE;
    loop {
        let mut child = locked(child);
        let ended = !matches!(child.exited(), Ok(false));
        if ended || Instant::now() >= deadline {
            let _ = child.signal_group(Signal::Kill);
            let _ = child.wait();
            return;
        }
        drop(child);
        thread::sleep(POLL);
    }
}

/// Ends a run that has not started reading, and reaps it.
fn end_now(child: &mut Child) {
    let _ = child.signal_group(Signal::Kill);
    let _ = child.wait();
}

fn locked(child: &Mutex<Child>) -> MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads `stream` in pieces on a thread of its own, sending each, then that it closed.
fn reader(mut stream: impl Read + Send + 'static, sent: SyncSender<Piece>) -> io::Result<()> {
    thread::Builder::new()
        .name("oso-git-read".into())
        .spawn(move || {
            let mut piece = vec![0; PIECE];
            loop {
                match stream.read(&mut piece) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sent.send(Piece::Out(piece[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = sent.send(Piece::Closed);
        })
        .map(drop)
}

#[cfg(test)]
mod tests;
