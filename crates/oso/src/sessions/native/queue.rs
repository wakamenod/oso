//! The messages the person sends a native session, and when each goes (ADR 0165, step 2, the
//! author's Q1 (a)): at once while the session is idle, else held by oso until it is, shown grey
//! at the end of the transcript, taken back, edited in their place, dropped or moved there, and
//! sent one a turn. These are the rules; the caller says whether a turn runs and the process
//! does, and carries out what they come to.
//!
//! Must not depend on: threads, the stream or the UI.
//!
//! - oso writes to the session only while it is idle: a send while a turn runs, oso's or one it
//!   did not start, is queued, and so is one while the process does not run.
//! - At most [`MAX_QUEUED`] wait, and no message is longer than [`MAX_MESSAGE`] bytes or empty,
//!   an edit put back too.
//! - One goes at a time, the first, at each turn's end, and never one the person edits: the
//!   next waits until the edit is put back or given up.
//! - Nothing queued goes while oso waits for the sessions to be idle to exit; what the person
//!   sends then goes, as they asked.
//! - The queue pauses when the process ends, and when a delivery fails: nothing queued goes
//!   until the person sends again, which goes on with the queue, or sends nothing, which sends
//!   the next once the session is idle.

use std::collections::VecDeque;

use super::render::one_line;

/// The most messages that wait.
pub const MAX_QUEUED: usize = 20;

/// The longest message sent, in bytes.
pub const MAX_MESSAGE: usize = 64 << 10;

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
    paused: bool,
    /// The message the person edits in its place, by its id.
    editing: Option<u64>,
}

/// Where the session is, as the caller knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    /// A turn is under way: oso's, or one it did not start.
    pub busy: bool,
    /// Its process runs and takes messages.
    pub running: bool,
    /// oso waits for the sessions to be idle to exit: nothing queued goes, as it would start a
    /// turn the exit waits for again.
    pub leaving: bool,
}

/// What a send of the composer's text comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// Write this to the session now.
    Now(String),
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

    /// Whether nothing queued goes until the person sends again.
    #[cfg(test)]
    pub fn paused(&self) -> bool {
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
            if !(self.paused && turn.running && !self.waiting.is_empty()) {
                return Sent::Refused(Refusal::Empty);
            }
            self.paused = false;
            return match self.idle(turn) {
                Some(text) => Sent::Now(text),
                None => Sent::Resumed,
            };
        }
        if turn.running {
            self.paused = false;
            if !turn.busy {
                return Sent::Now(text);
            }
        }
        if self.waiting.len() >= MAX_QUEUED {
            return Sent::Refused(Refusal::Full);
        }
        self.push(text);
        if !turn.running {
            self.paused = true;
        }
        Sent::Queued(self.waiting.len())
    }

    /// The message to write now that `turn` has the session idle: the first, out of the queue,
    /// unless the queue is paused or the person edits it.
    pub fn idle(&mut self, turn: Turn) -> Option<String> {
        if turn.busy || !turn.running || turn.leaving || self.paused {
            return None;
        }
        let first = self.waiting.front()?;
        if self.editing == Some(first.id) {
            return None;
        }
        self.waiting.pop_front().map(|queued| queued.text)
    }

    /// `text`, taken by [`Queue::idle`] or [`Queue::send`], was not written: it goes back first,
    /// and the queue waits for the person.
    pub fn failed(&mut self, text: String) {
        self.next_id += 1;
        let id = self.next_id;
        self.waiting.push_front(Queued { id, text });
        self.paused = true;
    }

    /// The process ended: nothing queued goes until the person sends.
    pub fn ended(&mut self) {
        self.paused = !self.waiting.is_empty();
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

    /// The queue's rows: a heading, then each message's first line, cut, the one edited marked;
    /// nothing while none waits.
    pub fn rows(&self) -> String {
        use std::fmt::Write as _;
        let mut text = String::new();
        let count = self.waiting.len();
        if count == 0 {
            return text;
        }
        let paused = if self.paused {
            " · paused: C-c C-c on an empty composer sends the next"
        } else {
            ""
        };
        let _ = writeln!(text, "Queued ({count}){paused}");
        for (at, queued) in self.waiting.iter().enumerate() {
            let first = queued.text.lines().next().unwrap_or("");
            let mut shown = one_line(first);
            if first.len() < queued.text.trim_end().len() && !shown.ends_with('…') {
                shown.push('…');
            }
            let editing = if self.editing == Some(queued.id) {
                " (editing)"
            } else {
                ""
            };
            let _ = writeln!(text, "  {}. {shown}{editing}", at + 1);
        }
        text
    }

    fn push(&mut self, text: String) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.waiting.push_back(Queued { id, text });
        id
    }

    fn at(&self, id: u64) -> Option<usize> {
        self.waiting.iter().position(|queued| queued.id == id)
    }
}

#[cfg(test)]
mod tests;
