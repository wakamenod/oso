//! A native session's interrupt (ADR 0165, "The route": tier 2 with tier 1 as its fallback;
//! step 2c), from the person's key to the turn's end: these are the rules, at the instants the
//! caller gives, over the control layer's word on whether the request goes. The caller writes
//! the request, sends the signal and resumes the process.
//!
//! Must not depend on: threads, the stream or the UI; the clock is the caller's.
//!
//! - On tier 2 an interrupt is the `interrupt` request, and the process stays. With no answer
//!   within [`ANSWER_WAIT`], an error, or the turn still under way [`END_WAIT`] after the
//!   answer, it falls back to tier 1, saying why in a log line.
//! - On tier 1 the process is interrupted with a signal, which ends the turn and then the
//!   process (measured, CLI 2.1.330); it is stopped if it has not ended within
//!   [`SIGNAL_WAIT`]. Either way it is resumed (`--resume`) once it ended.
//! - Where no signal reaches the CLI alone (Windows, where it would end at once and leave its
//!   tools running), there is no tier 1: an interrupt the request does not carry is given up,
//!   and the person told.
//! - One interrupt at a time: a second while one is under way changes nothing.

use std::time::{Duration, Instant};

use super::control::Control;
use super::wire::control::Request;

/// How long an `interrupt` request has to be answered.
pub const ANSWER_WAIT: Duration = Duration::from_secs(2);
/// How long the turn has to end once its interrupt was answered.
pub const END_WAIT: Duration = Duration::from_secs(5);
/// How long the process has to end once it was interrupted with a signal.
pub const SIGNAL_WAIT: Duration = Duration::from_secs(5);

/// What the caller does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// Log why the request fell back.
    Log(String),
    /// Write the `interrupt` request with this id.
    Request(u64),
    /// Interrupt the process with a signal: it is resumed once it ends.
    Signal,
    /// The signal went unheeded: stop the process, which is resumed once it ends.
    Stop,
    /// Nothing carries the interrupt here: tell the person, whose turn goes on.
    GaveUp,
}

/// Why an interrupt was not begun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotBegun {
    /// One is under way.
    Already,
    /// The request does not go, and no signal reaches the CLI alone here.
    NoWay,
}

/// One process's interrupt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupt {
    /// A signal reaches the CLI alone, and it ends its tools itself.
    signals: bool,
    state: Option<State>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// The request `id` was written, to be answered by then.
    Asked { id: u64, by: Instant },
    /// It was answered: the turn is to end by then.
    Answered { by: Instant },
    /// The process was signalled, to end by then.
    Signalled { by: Instant },
    /// It was stopped as well.
    Stopped,
}

impl Interrupt {
    /// No interrupt under way, of a process a signal reaches alone if `signals`.
    pub fn new(signals: bool) -> Self {
        Self {
            signals,
            state: None,
        }
    }

    /// An interrupt is under way.
    pub fn interrupting(&self) -> bool {
        self.state.is_some()
    }

    /// The process was signalled or stopped: once it ends, it is resumed.
    pub fn resumes(&self) -> bool {
        matches!(self.state, Some(State::Signalled { .. } | State::Stopped))
    }

    /// The person interrupts the turn under way, at `now`, `control` saying whether the
    /// request goes.
    ///
    /// # Errors
    ///
    /// [`NotBegun`] while one is under way, or when nothing carries it.
    pub fn begin(&mut self, control: &mut Control, now: Instant) -> Result<Then, NotBegun> {
        if self.state.is_some() {
            return Err(NotBegun::Already);
        }
        if control.has(Request::Interrupt) {
            let id = control.ask(Request::Interrupt);
            self.state = Some(State::Asked {
                id,
                by: now + ANSWER_WAIT,
            });
            return Ok(Then::Request(id));
        }
        if !self.signals {
            return Err(NotBegun::NoWay);
        }
        Ok(self.signal(now))
    }

    /// The request was not written: it falls back.
    pub fn not_written(&mut self, control: &mut Control, now: Instant) -> Vec<Then> {
        self.fall_back(control, now, "the interrupt request was not written")
    }

    /// The CLI answered request `id`, refused with `error` if `Some`, at `now`.
    pub fn answered(
        &mut self,
        control: &mut Control,
        id: u64,
        error: Option<&str>,
        now: Instant,
    ) -> Vec<Then> {
        let Some(State::Asked { id: asked, .. }) = self.state else {
            return Vec::new();
        };
        if asked != id {
            return Vec::new();
        }
        match error {
            None => {
                self.state = Some(State::Answered { by: now + END_WAIT });
                Vec::new()
            }
            Some(why) => self.fall_back(control, now, &format!("interrupt was refused: {why}")),
        }
    }

    /// The turn ended: an interrupt by request is over; one by signal waits for the process
    /// to end.
    pub fn turn_ended(&mut self) {
        if matches!(
            self.state,
            Some(State::Asked { .. } | State::Answered { .. })
        ) {
            self.state = None;
        }
    }

    /// When something is next due, if anything is.
    pub fn deadline(&self) -> Option<Instant> {
        match self.state? {
            State::Asked { by, .. } | State::Answered { by } | State::Signalled { by } => Some(by),
            State::Stopped => None,
        }
    }

    /// What is due at `now`, in order.
    pub fn due(&mut self, control: &mut Control, now: Instant) -> Vec<Then> {
        match self.state {
            Some(State::Asked { by, .. }) if now >= by => {
                self.fall_back(control, now, "interrupt was not answered")
            }
            Some(State::Answered { by }) if now >= by => {
                self.fall_back(control, now, "the turn went on after its interrupt")
            }
            Some(State::Signalled { by }) if now >= by => {
                self.state = Some(State::Stopped);
                vec![Then::Stop]
            }
            _ => Vec::new(),
        }
    }

    /// The request did not carry the interrupt, for reason `why`: a signal does, where one
    /// reaches the CLI alone; else it is given up.
    fn fall_back(&mut self, control: &mut Control, now: Instant, why: &str) -> Vec<Then> {
        if let Some(State::Asked { id, .. }) = self.state {
            control.forget(id);
        }
        let then = match self.signals {
            true => self.signal(now),
            false => {
                self.state = None;
                Then::GaveUp
            }
        };
        vec![Then::Log(why.to_owned()), then]
    }

    fn signal(&mut self, now: Instant) -> Then {
        self.state = Some(State::Signalled {
            by: now + SIGNAL_WAIT,
        });
        Then::Signal
    }
}

#[cfg(test)]
mod tests;
