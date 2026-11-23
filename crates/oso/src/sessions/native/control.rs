//! A native session's control layer, tier 2 of ADR 0165 ("The route"; step 2c): whether the
//! CLI speaks it, for which requests, the ids of oso's requests and which one an answer is to.
//! The requests themselves are their own modules' (`interrupt`, `usage`, `mode`), the lines
//! `wire::control`'s.
//!
//! Must not depend on: threads, the stream or the UI; the clock is the caller's.
//!
//! - `initialize` is written first, as the process starts. Tier 2 is on once it is answered
//!   with success; a request is then sent unless `system/init` says the CLI lacks it, as
//!   [`RECEIPT`] does for `interrupt`. Until the first `system/init`, which comes with the
//!   first turn's start, the answer is enough: the request has its own fallback. An error, or
//!   no answer within [`INIT_WAIT`], leaves the session on tier 1; a success that comes later
//!   still turns tier 2 on.
//! - Why tier 2 is off is a log line, said once a process; why one request fell back is its
//!   own module's to say.
//! - An answer whose id is no request of oso's that waits is no answer, and neither is an
//!   answer to `initialize` after one that succeeded: what it lists, its commands, models and
//!   mode, is the first answer's.

use std::time::{Duration, Instant};

use super::wire::control::{Answer, INITIALIZE, Request};

/// How long `initialize` has to be answered.
pub const INIT_WAIT: Duration = Duration::from_secs(10);
/// What `system/init`'s capabilities say when the CLI answers an interrupt.
pub const RECEIPT: &str = "interrupt_receipt_v1";
/// The most requests that wait for an answer: those past it are forgotten, oldest first.
const MAX_WAITING: usize = 16;

/// One process's control layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    initialize: Initialize,
    /// Whether the last `system/init` listed [`RECEIPT`], once one came.
    receipt: Option<bool>,
    last_id: u64,
    /// oso's requests that wait for an answer, by id.
    waiting: Vec<(u64, Request)>,
    /// Why tier 2 was found off, said once.
    said: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Initialize {
    /// Written, to be answered by then.
    Waiting(Instant),
    Answered,
    Refused,
}

/// What an answer is to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answered {
    /// `initialize`'s, and the log line it comes to, if any.
    Initialize(Option<String>),
    /// The request `id` of kind `request`'s, and why it failed, if it did.
    Request {
        request: Request,
        id: u64,
        error: Option<String>,
    },
    /// No request of oso's that waits.
    Other,
}

impl Control {
    /// The control layer of a process started at `now`, whose `initialize` was written.
    pub fn started(now: Instant) -> Self {
        Self {
            initialize: Initialize::Waiting(now + INIT_WAIT),
            receipt: None,
            last_id: INITIALIZE,
            waiting: Vec::new(),
            said: false,
        }
    }

    /// Whether `request` goes as a request now: else it falls back to tier 1.
    pub fn has(&self, request: Request) -> bool {
        let initialized = self.initialize == Initialize::Answered;
        match request {
            Request::Initialize => false,
            Request::Interrupt => initialized && self.receipt != Some(false),
            Request::GetUsage | Request::SetPermissionMode(_) | Request::RemoteControl => {
                initialized
            }
        }
    }

    /// `system/init` listed `capabilities`; the log line that comes to, if any.
    pub fn system_init(&mut self, capabilities: &[String]) -> Option<String> {
        let receipt = capabilities.iter().any(|name| name == RECEIPT);
        self.receipt = Some(receipt);
        let missing = self.initialize == Initialize::Answered && !receipt;
        missing
            .then(|| self.say(format!("system/init does not list {RECEIPT}")))
            .flatten()
    }

    /// The id of a request of kind `request` about to be written, which waits for its answer.
    pub fn ask(&mut self, request: Request) -> u64 {
        self.last_id += 1;
        self.ask_as(self.last_id, request);
        self.last_id
    }

    /// A request of kind `request` about to be written with `id`, the caller's, which waits for
    /// its answer: one no line can name before it is written, as `id` is random (ADR 0181).
    pub fn ask_as(&mut self, id: u64, request: Request) {
        if self.waiting.len() == MAX_WAITING {
            self.waiting.remove(0);
        }
        self.waiting.push((id, request));
    }

    /// Request `id` is answered no more: it went unanswered too long, or was not written.
    pub fn forget(&mut self, id: u64) {
        self.waiting.retain(|(waiting, _)| *waiting != id);
    }

    /// What `answer` is to.
    pub fn answered(&mut self, answer: &Answer) -> Answered {
        let Some(id) = answer.id else {
            return Answered::Other;
        };
        if id == INITIALIZE {
            // Only the first: a later one is no answer, so that what it lists is not taken.
            if self.initialize == Initialize::Answered {
                return Answered::Other;
            }
            return Answered::Initialize(self.initialized(answer.error.as_deref()));
        }
        let Some(at) = self.waiting.iter().position(|(waiting, _)| *waiting == id) else {
            return Answered::Other;
        };
        let (id, request) = self.waiting.remove(at);
        Answered::Request {
            request,
            id,
            error: answer.error.clone(),
        }
    }

    /// `initialize` was answered, refused with `error` if `Some`. An answer after its wait is
    /// still taken: the CLI speaks the protocol.
    fn initialized(&mut self, error: Option<&str>) -> Option<String> {
        match error {
            None => {
                self.initialize = Initialize::Answered;
                None
            }
            Some(why) => {
                self.initialize = Initialize::Refused;
                self.say(format!("initialize was refused: {why}"))
            }
        }
    }

    /// When `initialize`'s wait ends, while it waits.
    pub fn deadline(&self) -> Option<Instant> {
        match self.initialize {
            Initialize::Waiting(by) => Some(by),
            Initialize::Answered | Initialize::Refused => None,
        }
    }

    /// What is due at `now`: the log line of an `initialize` not answered in time.
    pub fn due(&mut self, now: Instant) -> Option<String> {
        let Initialize::Waiting(by) = self.initialize else {
            return None;
        };
        if now < by {
            return None;
        }
        self.initialize = Initialize::Refused;
        self.say("initialize was not answered".to_owned())
    }

    /// `why`, the first time tier 2 is found off.
    fn say(&mut self, why: String) -> Option<String> {
        (!std::mem::replace(&mut self.said, true)).then_some(why)
    }
}

#[cfg(test)]
pub(super) mod tests;
