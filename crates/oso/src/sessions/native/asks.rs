//! The permission requests of native sessions' bridges, from their coming to their answer (ADR
//! 0165, step 2): each waits until its call is found in its session's stream, is asked one at a
//! time, the oldest first, and is answered once its question closed.
//!
//! Must not depend on: threads, sockets, the editor or the UI; the app finds the calls, opens
//! the questions and sends the replies.
//!
//! - At most one request waits for each connection, and so for each running native session
//!   (each holds one connection to the sessions' socket, whose server takes one request at a
//!   time); a second is refused, so the queue stays as long as the sessions are many.
//! - A call is asked about once: a request for a call that waits already, or that was answered
//!   lately, is refused.
//! - A request whose call has not shown within [`FIND_WITHIN`] is denied unasked, and so is one
//!   whose call stopped waiting, as it got its result, before it was asked.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use oso_core::minibuffer::PromptId;

use crate::channel::Connection;
use crate::channel::protocol::permission::Call;
use crate::sessions::worker::SessionKey;

/// How long a request waits for its call to show in its session's stream: the CLI writes the
/// call's message before it asks, as measured (ADR 0165, step 2).
pub const FIND_WITHIN: Duration = Duration::from_secs(2);

/// How many answered calls are kept, so that a request for one again is refused.
const MAX_ANSWERED: usize = 64;

/// The requests not answered yet, and the calls answered lately.
#[derive(Debug)]
pub struct Asks<R> {
    waiting: VecDeque<Ask<R>>,
    answered: VecDeque<(SessionKey, String)>,
}

impl<R> Default for Asks<R> {
    fn default() -> Self {
        Self {
            waiting: VecDeque::new(),
            answered: VecDeque::new(),
        }
    }
}

/// One request, and what replies to it.
#[derive(Debug)]
pub struct Ask<R> {
    pub from: Connection,
    pub key: SessionKey,
    pub call: Call,
    pub came: Instant,
    /// What the person is asked, once the call was found.
    pub question: Option<Question>,
    /// The question's prompt, once it is open.
    pub asked: Option<PromptId>,
    pub reply: R,
}

/// What the person is asked about a call, and what `?` shows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub label: String,
    pub details: String,
}

/// What the app finds of a request's call.
#[derive(Debug)]
pub enum Look {
    /// It waits in the stream, and says what the request says: the question about it.
    Found(Question),
    /// It still waits, and was found before.
    Waits,
    /// No call of that id and tool waits in the stream.
    NotWaiting,
    /// It waits, but the request's input says something else.
    Differs,
    /// The session is gone.
    Gone,
}

/// Why a request is refused as it comes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Its connection has a request waiting already.
    Twice,
    /// Its call waits already, or was answered lately.
    Again,
}

/// Why a request is denied unasked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    NotFound,
    NoLonger,
    Differs,
    Gone,
}

/// What [`Asks::look`] came to for a request.
#[derive(Debug)]
pub enum Outcome<R> {
    /// The call of a request of this session was found: it waits for the person now.
    Found(SessionKey),
    /// The request is denied unasked; whether it had been found.
    Denied(Ask<R>, Denied, bool),
}

impl<R> Asks<R> {
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Why a request from `from` of session `key` about the call `id` is refused, if it is:
    /// its connection has one waiting, or its call was asked about.
    pub fn refuses(&self, from: Connection, key: SessionKey, id: &str) -> Option<Refused> {
        if self.waiting.iter().any(|waiting| waiting.from == from) {
            return Some(Refused::Twice);
        }
        let again = (self.waiting.iter())
            .any(|waiting| waiting.key == key && waiting.call.tool_use_id == id)
            || (self.answered.iter()).any(|(asked, answered)| *asked == key && answered == id);
        again.then_some(Refused::Again)
    }

    /// Takes `ask`, which [`Asks::refuses`] does not refuse.
    pub fn push(&mut self, ask: Ask<R>) {
        debug_assert!(
            self.refuses(ask.from, ask.key, &ask.call.tool_use_id)
                .is_none()
        );
        self.waiting.push_back(ask);
    }

    /// When the first request whose call has not shown is denied, if one waits.
    pub fn deadline(&self) -> Option<Instant> {
        (self.waiting.iter())
            .filter(|ask| ask.question.is_none())
            .map(|ask| ask.came + FIND_WITHIN)
            .min()
    }

    /// Looks for each request's call not asked yet with `find`, as of `now`: a found call's
    /// request waits for the person, and one denied leaves the queue.
    pub fn look(&mut self, now: Instant, mut find: impl FnMut(&Ask<R>) -> Look) -> Vec<Outcome<R>> {
        let mut outcomes = Vec::new();
        if self.waiting.is_empty() {
            return outcomes;
        }
        let mut kept = VecDeque::with_capacity(self.waiting.len());
        for mut ask in self.waiting.drain(..) {
            if ask.asked.is_some() {
                kept.push_back(ask);
                continue;
            }
            let found = ask.question.is_some();
            let denied = match find(&ask) {
                Look::Found(question) if !found => {
                    ask.question = Some(question);
                    outcomes.push(Outcome::Found(ask.key));
                    None
                }
                Look::Found(_) | Look::Waits => None,
                Look::NotWaiting if found => Some(Denied::NoLonger),
                Look::NotWaiting if now.saturating_duration_since(ask.came) >= FIND_WITHIN => {
                    Some(Denied::NotFound)
                }
                Look::NotWaiting => None,
                Look::Differs => Some(Denied::Differs),
                Look::Gone => Some(Denied::Gone),
            };
            match denied {
                Some(why) => outcomes.push(Outcome::Denied(ask, why, found)),
                None => kept.push_back(ask),
            }
        }
        self.waiting = kept;
        outcomes
    }

    /// The oldest request found and not asked, unless one is asked now.
    pub fn next(&mut self) -> Option<&mut Ask<R>> {
        if self.waiting.iter().any(|ask| ask.asked.is_some()) {
            return None;
        }
        (self.waiting.iter_mut()).find(|ask| ask.question.is_some())
    }

    /// The request asked now.
    pub fn open(&self) -> Option<&Ask<R>> {
        self.waiting.iter().find(|ask| ask.asked.is_some())
    }

    /// The request asked now, if its question is no longer `open`: out of the queue, and its
    /// call kept as answered.
    pub fn take_closed(&mut self, open: impl Fn(PromptId) -> bool) -> Option<Ask<R>> {
        let index = (self.waiting.iter()).position(|ask| ask.asked.is_some_and(|id| !open(id)))?;
        let ask = self.waiting.remove(index)?;
        if self.answered.len() == MAX_ANSWERED {
            self.answered.pop_front();
        }
        (self.answered).push_back((ask.key, ask.call.tool_use_id.clone()));
        Some(ask)
    }

    /// Takes the request [`Asks::next`] gives out of the queue, as when its question could not
    /// open.
    pub fn take_next(&mut self) -> Option<Ask<R>> {
        let index =
            (self.waiting.iter()).position(|ask| ask.question.is_some() && ask.asked.is_none())?;
        self.waiting.remove(index)
    }

    /// Takes the requests `gone` says are gone out of the queue.
    pub fn remove_where(&mut self, gone: impl Fn(&Ask<R>) -> bool) -> Vec<Ask<R>> {
        if !self.waiting.iter().any(&gone) {
            return Vec::new();
        }
        let (removed, kept): (VecDeque<_>, VecDeque<_>) =
            self.waiting.drain(..).partition(|ask| gone(ask));
        self.waiting = kept;
        removed.into()
    }
}

#[cfg(test)]
mod tests;
