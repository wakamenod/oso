//! A terminal's thread over a restart (ADR 0120, step 11; ADR 0133). Exporting stops reading
//! the PTY at a chunk's end, writes the engine's state and hands back a second descriptor for
//! the master; until a resume or a release the thread still writes input and serves the UI,
//! but reads nothing and applies no resize, so the state stays the terminal's; it reads the
//! PTY's flags still, so that a password prompt opened meanwhile is seen. A release ends
//! the thread with nothing hung up or reaped. An adopted terminal restores its engine, sends
//! its first snapshot and keeps what is asked until its master comes, then reads where the
//! instance before stopped.
//!
//! Must not depend on: the UI thread's state; it shares only the channels and `Shared`.

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant, SystemTime};

use oso_platform::process::pty::Master;
use oso_platform::process::start_time;

use super::{Channels, Running, send_end};
use crate::engine::Engine;
use crate::engine::ghostty::Ghostty;
use crate::engine::ghostty::carry::{self, Encoding as Written, NotEncoded};
use crate::event::End;
use crate::limits;
use crate::policy;
use crate::snapshot::Snapshot;
use crate::terminal::Request;
use crate::terminal::handoff::{Adopt, Encoding, Exported, NotExported, Request as Asked};

/// Whether a terminal exported for the same library is written as the library's snapshot.
const SNAPSHOTS: bool = false;

/// How often input waiting to be written is tried again while the thread reads nothing.
const WRITE_AGAIN: Duration = Duration::from_millis(10);

/// How often the PTY's flags are read while the thread reads nothing, for a password prompt.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// The most requests an adopted terminal keeps until its master comes; input past them is
/// dropped and its share of the queue given back.
const KEPT_MAX: usize = 4096;

/// What the thread keeps for a restart.
#[derive(Default)]
pub(in crate::terminal) struct Handoff {
    /// Since when it reads nothing, after an export.
    paused: Option<Instant>,
    /// It let the terminal go to the next instance.
    released: bool,
    /// When an adopted terminal's child started, as the instance before read it: the id may
    /// be another process's by now, so it is not read again.
    started: Option<SystemTime>,
}

impl Handoff {
    pub(super) fn paused(&self) -> bool {
        self.paused.is_some()
    }

    pub(super) fn released(&self) -> bool {
        self.released
    }
}

impl Running<'_> {
    /// Serves a restart's request; returns whether nothing more may be served after it.
    pub(super) fn serve_handoff(&mut self, request: Asked) -> bool {
        match request {
            Asked::Export {
                library,
                budget,
                reply,
            } => {
                let answer = self.export(&library, &budget);
                if reply.send(answer).is_ok() {
                    (self.channels.wake)();
                }
            }
            Asked::Resume => self.resume(),
            Asked::Release => {
                self.handoff.released = true;
                return true;
            }
            // Only an adopted terminal waiting for its master takes these.
            Asked::Attach(_) | Asked::Abandon => {}
        }
        false
    }

    /// Writes the terminal's state and stops reading.
    fn export(
        &mut self,
        library: &str,
        budget: &std::sync::atomic::AtomicUsize,
    ) -> Result<Exported, NotExported> {
        // The library's snapshot is off until a patch to the library (ADR 0120, step 11 as
        // built): every terminal travels as escape sequences.
        let written = if SNAPSHOTS && library == carry::library() {
            Written::Snapshot
        } else {
            Written::Escapes
        };
        let state = self
            .engine
            .encode(written, budget)
            .map_err(|why| match why {
                NotEncoded::TooLarge => NotExported::TooLarge,
                NotEncoded::Refused(code) => NotExported::Failed(io::Error::other(format!(
                    "the terminal library could not write the terminal ({code})"
                ))),
            })?;
        let master = self.master.try_clone_fd().map_err(NotExported::Failed)?;
        let pid = match self.channels.shared.pid.load(Ordering::Acquire) {
            0 => None,
            pid => Some(pid),
        };
        // An own child is not reaped yet, so its id is still its own.
        let started = self
            .handoff
            .started
            .or_else(|| pid.and_then(|pid| start_time(pid).ok().flatten()));
        self.handoff.paused = Some(Instant::now());
        Ok(Exported {
            state,
            encoding: match written {
                Written::Snapshot => Encoding::Snapshot,
                Written::Escapes => Encoding::Escapes,
            },
            master,
            pid,
            started,
            size: self.next.size,
            echo_off: self.echo.echo_off(),
        })
    }

    /// Reads again after an export.
    fn resume(&mut self) {
        if self.handoff.paused.take().is_some() {
            self.channels.shared.resumes.fetch_add(1, Ordering::AcqRel);
            self.touched = true;
        }
    }

    /// Serves the UI and writes input while the thread reads nothing, until a resume, a
    /// release, the handle's end or `limits::PAUSE_MAX`. Returns false once released.
    pub(super) fn pause(&mut self) -> bool {
        while let Some(since) = self.handoff.paused {
            if self.handoff.released {
                return false;
            }
            if self.detached {
                return true;
            }
            let left = limits::PAUSE_MAX.saturating_sub(since.elapsed());
            if left.is_zero() {
                log::info!("a terminal reads again: no restart came for it");
                self.resume();
                break;
            }
            // A failed write ends the terminal once it reads again.
            if self.write().is_err() {
                self.resume();
                break;
            }
            // A password prompt that opens meanwhile holds the restart: the flags are read
            // though the output is not, and the UI hears of it at once.
            self.look();
            self.publish();
            let waiting = !self.input.is_empty() || !self.replies.is_empty();
            let wait = left.min(if waiting { WRITE_AGAIN } else { LOOK_EVERY });
            match self.channels.requests.recv_timeout(wait) {
                Ok(request) => {
                    self.touched = true;
                    if !self.serve_request(request) {
                        self.serve_requests();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.detached = true,
            }
        }
        !self.handoff.released
    }
}

/// Runs a terminal the instance before exported, from its restored engine to its end, and
/// sends the end as the last snapshot's event.
pub(in crate::terminal) fn adopt(adopt: Adopt, hold_limit: Duration, channels: Channels<'_>) {
    let size = limits::clamp(adopt.size);
    let mut last = Snapshot::new(size);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let Adopt {
            state,
            encoding,
            scrollback,
            palette,
            pid,
            started,
            ..
        } = adopt;
        let written = match encoding {
            // Until the snapshot is on, its decoder reads nothing another instance sent
            // (ADR 0133, T-parse): such a terminal is not restored.
            Encoding::Snapshot if !SNAPSHOTS => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "this build restores no snapshot",
                ));
            }
            Encoding::Snapshot => Written::Snapshot,
            Encoding::Escapes => Written::Escapes,
        };
        let scrollback = scrollback.min(limits::SCROLLBACK_MAX);
        let mut engine = Ghostty::restore(written, &state, size, scrollback, policy::XTVERSION)
            .ok_or_else(|| io::Error::other("the terminal could not be restored"))?;
        drop(state);
        engine.set_palette(&palette);
        // The child's id is set before the first snapshot is sent, so a watcher that sees the
        // snapshot already sees the pid (the host asks for it as soon as the buffer appears).
        let shared = channels.shared;
        shared.pid.store(pid.unwrap_or(0), Ordering::Release);
        let mut first = Snapshot::new(size);
        engine.snapshot(&mut first, &mut Vec::new());
        channels.shared.in_flight.store(true, Ordering::Release);
        if channels.snapshots.send(first).is_ok() {
            (channels.wake)();
        }
        let Some((master, kept)) = wait_for_master(channels)? else {
            shared.pid.store(0, Ordering::Release);
            return Ok(Some(End::Closed));
        };
        let mut running = Running::with(master, None, engine, size, hold_limit, channels);
        running.handoff.started = started;
        for request in kept {
            running.serve_request(request);
        }
        let end = running.run();
        channels.shared.pid.store(0, Ordering::Release);
        if running.released() {
            return Ok(None);
        }
        last = running.finish();
        Ok(Some(end))
    }));
    // Whatever ended it, the id is no longer said to be the terminal's.
    channels.shared.pid.store(0, Ordering::Release);
    send_end(outcome, last, channels);
}

/// Waits for an adopted terminal's master, keeping what is asked meanwhile. `None` if the
/// terminal was hung up, or its handle dropped, first.
fn wait_for_master(channels: Channels<'_>) -> io::Result<Option<(Master, Vec<Request>)>> {
    let mut kept = Vec::new();
    loop {
        match channels.requests.recv() {
            Ok(Request::Handoff(Asked::Attach(fd))) => {
                return Ok(Some((Master::from_fd(fd)?, kept)));
            }
            Ok(Request::Handoff(Asked::Abandon)) => {
                return Err(io::Error::other(
                    "the terminal stayed with the instance before",
                ));
            }
            Ok(Request::HangUp) | Err(_) => return Ok(None),
            Ok(Request::Handoff(_)) => {}
            Ok(Request::Input(input)) if kept.len() >= KEPT_MAX => {
                channels.shared.release(input.reserved());
            }
            Ok(request) => {
                if kept.len() < KEPT_MAX {
                    kept.push(request);
                }
            }
        }
    }
}
