//! The messages the person sends a native session, and when each goes (ADR 0165, step 2, the
//! author's Q1 (a)): at once while the session is idle, else held by oso until it is, shown grey
//! at the end of the transcript, taken back, edited in their place, dropped or moved there, and
//! sent one a turn. These are the rules; the caller says where the session is, and carries out
//! what they come to. `render::queue_rows` draws them.
//!
//! Must not depend on: threads, the stream or the UI.
//!
//! - oso writes to the session only while it is idle: a send while a turn runs, oso's or one it
//!   did not start, is queued, and so is one while the process starts or has ended.
//! - At most [`MAX_QUEUED`] wait, and no message is longer than [`MAX_MESSAGE`] bytes or empty,
//!   an edit put back too. A refused send changes nothing.
//! - One goes at a time, the first, once the session is idle, and never one the person edits:
//!   the next waits until the edit is put back or given up.
//! - Nothing queued goes while oso waits for the sessions to be idle to exit; what the person
//!   sends then goes, as they asked.
//! - The queue pauses when the process ends, and when a delivery fails, the person's or the
//!   queue's: nothing queued goes until the person sends again, which goes on with the queue,
//!   or, once it runs again, sends nothing, which sends the next once the session is idle.

use std::collections::VecDeque;

pub use super::super::stream::MAX_MESSAGE;

/// The most messages that wait.
pub const MAX_QUEUED: usize = 20;

/// A message that waits, by an id of its own, as its row is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub id: u64,
    pub text: String,
}

/// The queue of one session.
#[derive(Debug, Default)]
pub struct Queue {
    waiting: VecDeque<Queued>,
    next_id: u64,
    paused: Option<Pause>,
    /// The message the person edits in its place, by its id.
    editing: Option<u64>,
}

/// Why nothing queued goes until the person sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// The process ended.
    Ended,
    /// A message was not written, or the process runs again after it ended: an empty send
    /// goes on.
    Held,
}

/// Where the session's process is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    /// Being started: what is sent waits for its first turn's end.
    Starting,
    /// It runs and takes messages.
    Running,
    /// It ended, or never ran here.
    Ended,
}

/// Where the session is, as the caller knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    /// A turn is under way: oso's, or one it did not start.
    pub busy: bool,
    pub run: Run,
    /// oso waits for the sessions to be idle to exit: nothing queued goes, as it would start a
    /// turn the exit waits for again.
    pub leaving: bool,
}

/// What a send of the composer's text comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// Write the person's text to the session now; it stays in the composer until it went.
    Now(String),
    /// Write the queue's first now, the person having sent nothing.
    Next(String),
    /// Queued, this many waiting now.
    Queued(usize),
    /// The edited message is back in its place.
    PutBack,
    /// The paused queue goes on, once the turn under way ends.
    Resumed,
    /// Nothing changes.
    Refused(Refusal),
}

/// Why a send was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    TooLong,
    Full,
}

/// Which way a message moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
}

impl Queue {
    /// The messages that wait, the next first.
    pub fn waiting(&self) -> impl ExactSizeIterator<Item = &Queued> {
        self.waiting.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Why nothing queued goes until the person sends, if it does not.
    pub fn paused(&self) -> Option<Pause> {
        self.paused
    }

    /// The message the person edits in its place, if one is.
    pub fn editing(&self) -> Option<u64> {
        self.editing
    }

    /// The person sends `text` as `turn` has the session: written now, queued, put back in
    /// place of the message edited, or refused.
    pub fn send(&mut self, text: String, turn: Turn) -> Sent {
        if text.len() > MAX_MESSAGE {
            return Sent::Refused(Refusal::TooLong);
        }
        let empty = text.trim().is_empty();
        if let Some(id) = self.editing {
            if empty {
                return Sent::Refused(Refusal::Empty);
            }
            self.editing = None;
            if let Some(at) = self.at(id) {
                self.waiting[at].text = text;
            }
            return Sent::PutBack;
        }
        if empty {
            let resumes = self.paused.is_some() && turn.run == Run::Running;
            if !resumes || self.waiting.is_empty() {
                return Sent::Refused(Refusal::Empty);
            }
            self.paused = None;
            return match self.idle(turn) {
                Some(text) => Sent::Next(text),
                None => Sent::Resumed,
            };
        }
        let now = turn.run == Run::Running && !turn.busy;
        if !now && self.waiting.len() >= MAX_QUEUED {
            return Sent::Refused(Refusal::Full);
        }
        // The person's send goes on with a paused queue, while the process runs.
        if turn.run != Run::Ended {
            self.paused = None;
        }
        if now {
            return Sent::Now(text);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.waiting.push_back(Queued { id, text });
        if turn.run == Run::Ended {
            self.paused = Some(Pause::Ended);
        }
        Sent::Queued(self.waiting.len())
    }

    /// The message to write now that `turn` has the session idle: the first, out of the queue,
    /// unless the queue is paused or the person edits it.
    pub fn idle(&mut self, turn: Turn) -> Option<String> {
        if turn.busy || turn.run != Run::Running || turn.leaving || self.paused.is_some() {
            return None;
        }
        let first = self.waiting.front()?;
        if self.editing == Some(first.id) {
            return None;
        }
        self.waiting.pop_front().map(|queued| queued.text)
    }

    /// What [`Queue::send`] or [`Queue::idle`] gave was not written: the queue's own goes back
    /// first (`Some`), the person's stays in the composer (`None`), and the queue waits for
    /// the person either way.
    pub fn failed(&mut self, queued: Option<String>) {
        if let Some(text) = queued {
            self.next_id += 1;
            let id = self.next_id;
            self.waiting.push_front(Queued { id, text });
        }
        self.paused = Some(Pause::Held);
    }

    /// The process runs again after it ended (a resume): the queue still waits for the
    /// person, whose empty send now goes on with it.
    pub fn resumed(&mut self) {
        if self.paused == Some(Pause::Ended) {
            self.paused = Some(Pause::Held);
        }
    }

    /// The process ended: nothing queued goes until the person sends.
    pub fn ended(&mut self) {
        self.paused = (!self.waiting.is_empty()).then_some(Pause::Ended);
    }

    /// Takes message `id` out, for the composer.
    pub fn take_back(&mut self, id: u64) -> Option<String> {
        let at = self.at(id)?;
        if self.editing == Some(id) {
            self.editing = None;
        }
        self.waiting.remove(at).map(|queued| queued.text)
    }

    /// Message `id`'s text, for the composer to edit while it keeps its place.
    pub fn edit(&mut self, id: u64) -> Option<String> {
        let at = self.at(id)?;
        self.editing = Some(id);
        Some(self.waiting[at].text.clone())
    }

    /// The edit is given up: the message stays as it was.
    pub fn give_up_edit(&mut self) {
        self.editing = None;
    }

    /// Drops message `id`. Whether it waited.
    pub fn drop_one(&mut self, id: u64) -> bool {
        self.take_back(id).is_some()
    }

    /// Moves message `id` one place, as far as an end. Its place now, from 0, if it moved.
    pub fn shift(&mut self, id: u64, way: Move) -> Option<usize> {
        let at = self.at(id)?;
        let to = match way {
            Move::Up => at.checked_sub(1),
            Move::Down => (at + 1 < self.waiting.len()).then_some(at + 1),
        }?;
        self.waiting.swap(at, to);
        Some(to)
    }

    fn at(&self, id: u64) -> Option<usize> {
        self.waiting.iter().position(|queued| queued.id == id)
    }
}

#[cfg(test)]
mod tests;
