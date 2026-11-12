//! Documents no buffer owns (ADR 0122): a text that never changes, such as a review's side,
//! opened in the schedule beside the buffers, parsed and highlighted whole under ADR 0111's
//! limits, and closed by its owner. Also the lane they share with background buffers, its
//! order and its caps.
//!
//! Must not depend on: threads, the editor's buffers, windows or faces; it sees keys, ropes and
//! the verdict on where a text came from.
//!
//! - A text that may hold secrets is never opened: no tree, timing or highlight exists for it,
//!   as for a buffer that may hold secrets. The caller judges where the text came from and
//!   gives the verdict; once the source turns secret, it closes the document, which goes at
//!   once, as a dropped buffer does. A refused open closes a document open under the key too.
//! - Documents run only in the lane one thread at a time has, with background buffers, so a
//!   shown buffer always has a thread: shown documents first, then background buffers, then
//!   documents not shown; among documents, one that holds a tree first.
//! - At most [`MAX_DOCUMENTS`] are open, which bounds the bookkeeping under the lock, and at
//!   most [`MAX_STARTED`] hold a tree at once. A document that is done gives its tree to a
//!   thread to free; its slot stays, holding nothing, until it is closed.
//! - A checkout whose slot went while a thread had it still holds its lane and its tree until
//!   it is given back, so closing or replacing a document frees neither early.

use std::hash::Hash;
use std::ops::Range;

use oso_core::buffer::Verdict;
use oso_core::rope::Rope;

use super::{Scheduler, Slot, Waiting};
use crate::document::MAX_BYTES;
use crate::language::Language;

/// The most documents no buffer owns open at once.
pub(super) const MAX_DOCUMENTS: usize = 1024;

/// The most of them that hold a tree at once: a tree costs 35 to 60 times its text (ADR 0111).
pub(super) const MAX_STARTED: usize = 4;

/// Why [`Scheduler::open`] kept nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The verdict was not [`Verdict::Ordinary`]: the text is never parsed.
    MayHoldSecrets,
    /// The text is larger than [`MAX_BYTES`].
    TooLarge,
    /// As many documents as the schedule keeps are open: one may open once another closed.
    Full,
}

/// Whose a slot is.
pub(super) enum Kind {
    Buffer,
    /// A document no buffer owns: `place` keeps its place in the queue between slices, and
    /// `finished` says it is highlighted whole and holds nothing more.
    Unowned {
        place: u64,
        finished: bool,
    },
}

/// What a checkout whose slot went still holds, until it is given back.
pub(super) struct Stale {
    generation: u64,
    lane: bool,
    tree: bool,
}

impl Stale {
    pub(super) fn of(slot: &Slot) -> Self {
        Self {
            generation: slot.generation,
            lane: slot.in_lane(),
            tree: slot.holds_tree(),
        }
    }
}

impl Slot {
    pub(super) fn is_unowned(&self) -> bool {
        matches!(self.kind, Kind::Unowned { .. })
    }

    /// Whether it is a document no buffer owns with work left: not highlighted whole yet.
    fn is_unfinished(&self) -> bool {
        matches!(
            self.kind,
            Kind::Unowned {
                finished: false,
                ..
            }
        )
    }

    /// Whether it runs in the lane one thread at a time has: anything but a shown buffer.
    pub(super) fn in_lane(&self) -> bool {
        self.is_unowned() || self.shown.is_empty()
    }

    /// Whether it is a document no buffer owns that holds a tree, or is making one.
    pub(super) fn holds_tree(&self) -> bool {
        self.is_unfinished() && (self.checked_out || self.document.is_some())
    }

    /// Whether it is a document no buffer owns that would make a tree if taken now.
    pub(super) fn would_start(&self) -> bool {
        self.is_unfinished() && !self.checked_out && self.document.is_none()
    }

    /// Its rank in the lane, lower first: shown documents, background buffers, documents not
    /// shown; among documents, one that holds a tree first, so that it finishes and frees it;
    /// then the one waiting longest.
    pub(super) fn rank(&self) -> (u8, bool, Option<u64>) {
        let class = match (self.is_unowned(), self.shown.is_empty()) {
            (true, false) => 0,
            (false, _) => 1,
            (true, true) => 2,
        };
        (class, self.would_start(), self.queued)
    }
}

impl<Key: Copy + Eq + Hash> Scheduler<Key> {
    /// Opens a document no buffer owns (ADR 0122): `text` in `language`, parsed and highlighted
    /// whole, `shown` first. Refuses when `verdict` is not [`Verdict::Ordinary`], when `text`
    /// is larger than [`MAX_BYTES`], or when as many documents as the schedule keeps are open;
    /// a refusal keeps nothing, and closes a document open under `key`. A key that has a slot
    /// is replaced, as a job in a new language replaces one. O(1): nothing is parsed or freed
    /// here.
    ///
    /// Its reports carry epoch 0 and revision 0, as its text never changes, so a report of an
    /// earlier text under the same key looks the same: open each text under a key not used
    /// before.
    pub fn open(
        &mut self,
        key: Key,
        language: Language,
        verdict: Verdict,
        text: Rope,
        shown: Vec<Range<usize>>,
    ) -> Result<(), Refused> {
        let refused = if verdict.is_withheld() {
            Some(Refused::MayHoldSecrets)
        } else if text.len() > MAX_BYTES {
            Some(Refused::TooLarge)
        } else {
            let replaces = self.slots.get(&key).is_some_and(Slot::is_unowned);
            (!replaces && self.unowned >= MAX_DOCUMENTS).then_some(Refused::Full)
        };
        if let Some(refused) = refused {
            // What was open under the key is no less stale for the refusal.
            self.close(key);
            return Err(refused);
        }
        let place = self.next_order();
        self.generations += 1;
        let start = Waiting::Reset {
            text,
            revision: 0,
            epoch: 0,
        };
        let slot = Slot {
            queued: Some(place),
            kind: Kind::Unowned {
                place,
                finished: false,
            },
            ..Slot::fresh(self.generations, language, start, shown)
        };
        self.replace(key, slot);
        Ok(())
    }

    /// What windows show of an open document now: it is highlighted first, and ranks as shown.
    /// Nothing for a document that is done, nor for any other key.
    pub fn show(&mut self, key: Key, shown: Vec<Range<usize>>) {
        if let Some(slot) = self.slots.get_mut(&key)
            && slot.is_unfinished()
        {
            slot.shown = shown;
        }
    }

    /// Closes a document no buffer owns at once, as [`Self::drop_buffer`] drops a buffer: what
    /// waited for it never runs, a slice running on it gives back nothing, and its tree goes on
    /// the list to free. Nothing for any other key.
    pub fn close(&mut self, key: Key) {
        if self.slots.get(&key).is_some_and(Slot::is_unowned)
            && let Some(slot) = self.remove(&key)
        {
            self.retire(slot);
        }
    }

    /// Whether a thread has the lane: a background buffer or a document no buffer owns, its
    /// slot there or gone.
    pub(super) fn lane_busy(&self) -> bool {
        self.stale.iter().any(|stale| stale.lane)
            || (self.slots.values()).any(|slot| slot.checked_out && slot.in_lane())
    }

    /// Whether fewer than [`MAX_STARTED`] documents hold a tree, counting checkouts whose slot
    /// went.
    pub(super) fn room_for_a_tree(&self) -> bool {
        let stale = self.stale.iter().filter(|stale| stale.tree).count();
        let held = self.slots.values().filter(|slot| slot.holds_tree()).count();
        stale + held < MAX_STARTED
    }

    /// A checkout came back: if its slot had gone, what it held is free now.
    pub(super) fn settle(&mut self, generation: u64) {
        self.stale.retain(|stale| stale.generation != generation);
    }
}

#[cfg(test)]
mod tests;
