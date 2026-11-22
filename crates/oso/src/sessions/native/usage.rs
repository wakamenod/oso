//! A native session's `get_usage` request (ADR 0165, part 3a), over the control layer: asked
//! once a process, as soon as its `initialize` was answered with success (tier 2), so that a
//! resumed session shows its cost and its rate limit's window before its first turn. After
//! that, the stream's `result` and `rate_limit_event` keep the status line current. The caller
//! writes the request.
//!
//! Must not depend on: threads, the stream or the UI; the clock is the caller's.
//!
//! - Its answer comes through the control layer, as `interrupt`'s does. With no answer within
//!   [`ANSWER_WAIT`], the request is forgotten, and a later answer is none: the status line
//!   waits for the stream. A refusal and a wait that ended are logged.
//! - It is asked only once the process's stream is there to write it, which may be after the
//!   answer to `initialize` came (step 2c).
//! - On tier 1 it is never asked.

use std::time::{Duration, Instant};

use super::control::Control;
use super::wire::control::Request;

/// How long `get_usage` has to be answered.
pub const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// One process's `get_usage`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GetUsage {
    state: State,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    /// Not asked yet.
    #[default]
    Unasked,
    /// The request `id` was written, to be answered by then.
    Asked { id: u64, by: Instant },
    /// Answered, or given up.
    Done,
}

impl GetUsage {
    /// The id of the request to write now, at `now`, if it is due: once a process, when the
    /// control layer says it goes.
    pub fn ask(&mut self, control: &mut Control, now: Instant) -> Option<u64> {
        if self.state != State::Unasked || !control.has(Request::GetUsage) {
            return None;
        }
        let id = control.ask(Request::GetUsage);
        self.state = State::Asked {
            id,
            by: now + ANSWER_WAIT,
        };
        Some(id)
    }

    /// The request was not written: it is not asked again this process.
    pub fn not_written(&mut self, control: &mut Control) {
        if let State::Asked { id, .. } = self.state {
            control.forget(id);
        }
        self.state = State::Done;
    }

    /// What the control layer's answer to request `id` comes to, refused with `error` if
    /// `Some`: `None` when it is no answer to the request that waits, else `Ok` when its usage
    /// is to be taken, or the log line of its refusal.
    pub fn answered(&mut self, id: u64, error: Option<&str>) -> Option<Result<(), String>> {
        match self.state {
            State::Asked { id: asked, .. } if asked == id => {}
            _ => return None,
        }
        self.state = State::Done;
        Some(match error {
            None => Ok(()),
            Some(why) => Err(format!("get_usage was refused: {why}")),
        })
    }

    /// When its wait ends, while it waits.
    pub fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Asked { by, .. } => Some(by),
            State::Unasked | State::Done => None,
        }
    }

    /// What is due at `now`: the log line of a request not answered in time, which is
    /// forgotten.
    pub fn due(&mut self, control: &mut Control, now: Instant) -> Option<String> {
        let State::Asked { id, by } = self.state else {
            return None;
        };
        if now < by {
            return None;
        }
        control.forget(id);
        self.state = State::Done;
        Some("get_usage was not answered".to_owned())
    }
}

#[cfg(test)]
mod tests;
