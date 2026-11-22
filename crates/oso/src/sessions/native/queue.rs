//! The messages the person sent a native session while a turn ran, held by oso until the
//! session is idle (ADR 0165, step 2, the author's Q1 (a)): shown grey at the end of the
//! transcript, taken back, edited in their place, dropped or moved there, and sent one a turn.
//!
//! Must not depend on: threads, the stream or the UI; the caller says when the session is idle
//! and sends what this gives it.
//!
//! - At most [`MAX_QUEUED`] wait. One is sent at a time, the first, once the session is idle,
//!   and never one the person edits: the next waits until the edit is put back or given up.
//! - The queue pauses after an interrupt, and when the process ends, fails or is resumed:
//!   nothing queued goes until the person sends again, which goes on with the queue.

use std::collections::VecDeque;

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
    paused: bool,
    /// The message the person edits in its place, by its id.
    editing: Option<u64>,
}

/// Which way a message moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
}

/// Why a message was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

impl Queue {
    /// The messages that wait, the next first.
    pub fn waiting(&self) -> impl ExactSizeIterator<Item = &Queued> {
        self.waiting.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Whether nothing queued goes until the person sends again.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The message the person edits in its place, if one is.
    pub fn editing(&self) -> Option<u64> {
        self.editing
    }

    /// Queues `text` last, unless [`MAX_QUEUED`] wait. Its id.
    pub fn push(&mut self, text: String) -> Result<u64, Full> {
        if self.waiting.len() >= MAX_QUEUED {
            return Err(Full);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.waiting.push_back(Queued { id, text });
        Ok(id)
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

    /// The edit of the message edited ends with `text` in its place, or, if it is no longer
    /// queued, nothing changes. Whether it was put back.
    pub fn put_back(&mut self, text: String) -> bool {
        let Some(id) = self.editing.take() else {
            return false;
        };
        let Some(at) = self.at(id) else {
            return false;
        };
        self.waiting[at].text = text;
        true
    }

    /// The edit is given up: the message stays as it was.
    pub fn give_up_edit(&mut self) {
        self.editing = None;
    }

    /// Drops message `id`. Whether it waited.
    pub fn drop_one(&mut self, id: u64) -> bool {
        self.take_back(id).is_some()
    }

    /// Moves message `id` one place, as far as an end. Whether it moved.
    pub fn shift(&mut self, id: u64, way: Move) -> bool {
        let Some(at) = self.at(id) else {
            return false;
        };
        let to = match way {
            Move::Up => at.checked_sub(1),
            Move::Down => (at + 1 < self.waiting.len()).then_some(at + 1),
        };
        let Some(to) = to else {
            return false;
        };
        self.waiting.swap(at, to);
        true
    }

    /// Nothing queued goes until the person sends again.
    pub fn pause(&mut self) {
        self.paused = !self.waiting.is_empty();
    }

    /// The person sent: the queue goes on.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// The message to send now that the session is `idle`, out of the queue: the first, unless
    /// the queue is paused or the person edits it.
    pub fn next(&mut self, idle: bool) -> Option<String> {
        if !idle || self.paused {
            return None;
        }
        let first = self.waiting.front()?;
        if self.editing == Some(first.id) {
            return None;
        }
        self.waiting.pop_front().map(|queued| queued.text)
    }

    fn at(&self, id: u64) -> Option<usize> {
        self.waiting.iter().position(|queued| queued.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(queue: &Queue) -> Vec<&str> {
        queue.waiting().map(|queued| queued.text.as_str()).collect()
    }

    #[test]
    fn one_goes_a_turn_the_first_once_idle() {
        let mut queue = Queue::default();
        queue.push("a".into()).unwrap();
        queue.push("b".into()).unwrap();
        assert_eq!(queue.next(false), None, "a turn runs");
        assert_eq!(queue.next(true).as_deref(), Some("a"));
        assert_eq!(texts(&queue), ["b"]);
    }

    #[test]
    fn it_holds_at_most_its_most() {
        let mut queue = Queue::default();
        for n in 0..MAX_QUEUED {
            queue.push(n.to_string()).unwrap();
        }
        assert_eq!(queue.push("late".into()), Err(Full));
    }

    #[test]
    fn a_message_is_taken_back_dropped_and_moved_by_its_id() {
        let mut queue = Queue::default();
        let a = queue.push("a".into()).unwrap();
        let b = queue.push("b".into()).unwrap();
        let c = queue.push("c".into()).unwrap();
        assert!(queue.shift(c, Move::Up));
        assert_eq!(texts(&queue), ["a", "c", "b"]);
        assert!(!queue.shift(a, Move::Up), "at the top already");
        assert!(queue.drop_one(b));
        assert!(!queue.drop_one(b), "gone");
        assert_eq!(queue.take_back(a).as_deref(), Some("a"));
        assert_eq!(texts(&queue), ["c"]);
    }

    #[test]
    fn a_message_edited_keeps_its_place_and_holds_the_queue_back_while_first() {
        let mut queue = Queue::default();
        let a = queue.push("a".into()).unwrap();
        queue.push("b".into()).unwrap();
        assert_eq!(queue.edit(a).as_deref(), Some("a"));
        assert_eq!(queue.next(true), None, "the first is being edited");
        assert!(queue.put_back("a, better".into()));
        assert_eq!(queue.next(true).as_deref(), Some("a, better"));
        // An edit of one sent meanwhile puts nothing back.
        let b = queue.waiting().next().unwrap().id;
        queue.edit(b).unwrap();
        assert_eq!(queue.take_back(b).as_deref(), Some("b"));
        assert!(!queue.put_back("late".into()));
    }

    #[test]
    fn a_paused_queue_waits_for_the_persons_send() {
        let mut queue = Queue::default();
        queue.pause();
        assert!(!queue.paused(), "nothing to hold back");
        queue.push("a".into()).unwrap();
        queue.pause();
        assert_eq!(queue.next(true), None);
        queue.resume();
        assert_eq!(queue.next(true).as_deref(), Some("a"));
        let b = queue.push("b".into()).unwrap();
        assert_eq!(queue.take_back(b).as_deref(), Some("b"));
        assert!(queue.is_empty());
    }
}
