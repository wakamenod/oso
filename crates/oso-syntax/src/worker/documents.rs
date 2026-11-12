//! Documents no buffer owns (ADR 0122): a text that never changes, such as a review's side,
//! opened in the schedule beside the buffers, parsed and highlighted whole under ADR 0111's
//! limits, and closed by its owner.
//!
//! Must not depend on: threads, buffers, windows or faces; it sees keys, ropes and verdicts.
//!
//! - A text that may hold secrets is never opened: no tree, timing or highlight exists for it,
//!   as for a buffer that may hold secrets. The caller judges where the text came from and
//!   gives the verdict; once the source turns secret, it closes the document, which goes at
//!   once, as a dropped buffer does.
//! - Documents run only in the lane one thread at a time has, so a shown buffer always has a
//!   thread: shown documents first, then background buffers, then documents not shown.
//! - At most [`MAX_DOCUMENTS`] are open, which bounds the bookkeeping under the lock, and at
//!   most [`MAX_STARTED`] hold a tree at once. A document that is done gives its tree to a
//!   thread to free; its slot stays, holding nothing, until it is closed.

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
    Document {
        place: u64,
        finished: bool,
    },
}

impl<Key: Copy + Eq + Hash> Scheduler<Key> {
    /// Opens a document no buffer owns (ADR 0122): `text` in `language`, parsed and highlighted
    /// whole, `shown` first. Refuses, keeping nothing, when `verdict` is not
    /// [`Verdict::Ordinary`], when `text` is larger than [`MAX_BYTES`], or when as many
    /// documents as the schedule keeps are open. A key that has a slot is replaced, as a job in
    /// a new language replaces one. O(slots): nothing is parsed or freed here.
    ///
    /// Its reports carry epoch 0 and revision 0, as its text never changes.
    pub fn open(
        &mut self,
        key: Key,
        language: Language,
        verdict: Verdict,
        text: Rope,
        shown: Vec<Range<usize>>,
    ) -> Result<(), Refused> {
        if verdict.is_withheld() {
            return Err(Refused::MayHoldSecrets);
        }
        if text.len() > MAX_BYTES {
            return Err(Refused::TooLarge);
        }
        let replaces = self.slots.get(&key).is_some_and(Slot::is_document);
        let open = self
            .slots
            .values()
            .filter(|slot| slot.is_document())
            .count();
        if !replaces && open >= MAX_DOCUMENTS {
            return Err(Refused::Full);
        }
        let place = self.next_order();
        self.generations += 1;
        let slot = Slot {
            generation: self.generations,
            language,
            document: None,
            checked_out: false,
            revision: 0,
            epoch: 0,
            waiting: Waiting::Reset {
                text,
                revision: 0,
                epoch: 0,
            },
            shown,
            queued: Some(place),
            reported: None,
            kind: Kind::Document {
                place,
                finished: false,
            },
        };
        if let Some(old) = self.slots.insert(key, slot) {
            // Not freed here: the UI thread may be the caller.
            self.doomed.extend(old.document);
        }
        Ok(())
    }

    /// What windows show of an open document now: it is highlighted first, and ranks as shown.
    /// Nothing for a document that is done, nor for any other key.
    pub fn show(&mut self, key: Key, shown: Vec<Range<usize>>) {
        if let Some(slot) = self.slots.get_mut(&key)
            && matches!(
                slot.kind,
                Kind::Document {
                    finished: false,
                    ..
                }
            )
        {
            slot.shown = shown;
        }
    }

    /// Closes a document no buffer owns at once, as [`Self::drop_buffer`] drops a buffer: what
    /// waited for it never runs, a slice running on it gives back nothing, and its tree goes on
    /// the list to free. Nothing for any other key.
    pub fn close(&mut self, key: Key) {
        if self.slots.get(&key).is_some_and(Slot::is_document)
            && let Some(slot) = self.slots.remove(&key)
        {
            self.doomed.extend(slot.document);
        }
    }
}

#[cfg(test)]
mod tests;
