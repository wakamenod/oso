//! The global mark ring (ADR 0190), as Emacs's `global-mark-ring`: the places, a buffer and
//! an offset each, where marks were pushed in buffers other than the newest place's, and
//! `pop-global-mark` (`C-x C-SPC`) going back through them.
//!
//! Must not depend on: `command` or `keymap`, or the host's window.
//!
//! - It holds [`MARK_RING_SIZE`] places, the newest first; past that, the oldest goes.
//! - A mark is pushed through a window's selection set, which says so
//!   (`SelectionSet::take_pushed`); when a command ends, each window's set, and the place it
//!   last left, are asked, so that a push before a switch to another buffer counts too.
//! - A place follows its buffer's edits, undos and reloads as a marker does, as a window's
//!   jumps do (`window::Jumps`). A killed buffer's places go.
//! - `Debug` gives only how many places there are: an offset tells how long words are in a
//!   buffer that may hold secrets (ADR 0058).

use std::collections::VecDeque;
use std::fmt;

use super::Editor;
use crate::buffer::BufferId;
use crate::selection::{MARK_RING_SIZE, OffsetEdit, follow_sorted};
use crate::sum_tree::Bias;

/// What `pop-global-mark` says with no place to go to.
const NO_GLOBAL_MARK: &str = "No global mark set";

/// What it says to the agent when the newest place is in a buffer it may not be shown.
const NOT_FOR_AGENT: &str = "That place's buffer is not for the agent";

/// The places of the global mark ring, the newest first.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct GlobalMarks {
    places: VecDeque<(BufferId, usize)>,
}

impl fmt::Debug for GlobalMarks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlobalMarks")
            .field("places", &self.places.len())
            .finish()
    }
}

impl GlobalMarks {
    /// How many places it holds.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    /// Whether it holds no place.
    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// The places, the newest first. A caller that shows them leaves out those in a buffer
    /// that may hold secrets (ADR 0044).
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (BufferId, usize)> + '_ {
        self.places.iter().copied()
    }

    /// Keeps `place`, where a mark was pushed, as the newest, unless the newest is in the same
    /// buffer, as Emacs's `push-mark` does.
    pub(crate) fn push(&mut self, place: (BufferId, usize)) {
        if self
            .places
            .front()
            .is_some_and(|(newest, _)| *newest == place.0)
        {
            return;
        }
        if self.places.len() == MARK_RING_SIZE {
            self.places.pop_back();
        }
        self.places.push_front(place);
    }

    /// The newest place, without taking it.
    pub(crate) fn newest(&self) -> Option<(BufferId, usize)> {
        self.places.front().copied()
    }

    /// Moves the newest place to the end, as `pop-global-mark` does, so that going on goes
    /// round the ring.
    pub(crate) fn rotate(&mut self) {
        if let Some(newest) = self.places.pop_front() {
            self.places.push_back(newest);
        }
    }

    /// Lets the places in `buffer` go, as it is killed.
    pub(crate) fn forget(&mut self, buffer: BufferId) {
        self.places.retain(|(kept, _)| *kept != buffer);
    }

    /// Moves the places in `buffer` through `steps`, each a step's edits, as an Emacs marker:
    /// a place at an insertion stays before it. O(edits + places log places).
    pub(crate) fn follow(&mut self, buffer: BufferId, steps: &[Vec<OffsetEdit>]) {
        self.map_sorted(buffer, |offsets| {
            for edits in steps {
                follow_sorted(offsets, edits, Bias::Left);
            }
        });
    }

    /// Moves each place in `buffer` through `map`, as a reload moves a cursor.
    pub(crate) fn map(&mut self, buffer: BufferId, mut map: impl FnMut(usize) -> usize) {
        self.map_sorted(buffer, |offsets| {
            for offset in offsets {
                *offset = map(*offset);
            }
        });
    }

    /// Gives `change` the offsets of the places in `buffer`, ascending, on the stack, and puts
    /// back what it made of them.
    fn map_sorted(&mut self, buffer: BufferId, change: impl FnOnce(&mut [usize])) {
        let mut order = [(0usize, 0usize); MARK_RING_SIZE];
        let mut len = 0;
        for (index, (kept, offset)) in self.places.iter().enumerate() {
            if *kept == buffer && len < order.len() {
                order[len] = (*offset, index);
                len += 1;
            }
        }
        if len == 0 {
            return;
        }
        let order = &mut order[..len];
        order.sort_unstable();
        let mut offsets = [0usize; MARK_RING_SIZE];
        for (slot, (offset, _)) in offsets.iter_mut().zip(order.iter()) {
            *slot = *offset;
        }
        change(&mut offsets[..len]);
        for (offset, (_, index)) in offsets.iter().zip(order.iter()) {
            self.places[*index].1 = *offset;
        }
    }
}

impl Editor {
    /// The global mark ring.
    pub fn global_marks(&self) -> &GlobalMarks {
        &self.global_marks
    }

    /// Takes in the marks the command pushed, as it ends: in each tiled window, the place it
    /// last left, then the buffer it shows, the selected window last, so that the newest push
    /// is the newest place. A place is the primary cursor's mark.
    pub(super) fn note_pushed_marks(&mut self) {
        let tiled = self.tiled_count();
        let order = (0..tiled)
            .filter(|index| *index != self.selected)
            .chain((self.selected < tiled).then_some(self.selected));
        let mut pushed = Vec::new();
        for index in order {
            let window = &mut self.windows[index];
            if let Some((buffer, place)) = window.places_mut().latest_mut()
                && place.selections.take_pushed()
                && let Some(mark) = place.selections.primary().mark
            {
                pushed.push((buffer, mark));
            }
            let buffer = window.buffer();
            if window.selections.take_pushed()
                && let Some(mark) = window.selections.primary().mark
            {
                pushed.push((buffer, mark));
            }
        }
        for place in pushed {
            self.global_marks.push(place);
        }
    }

    /// `pop-global-mark`: shows the newest place's buffer in the selected window, with one
    /// cursor, from the primary, at the place, clipped to the text and to a display cluster's
    /// start, and moves that place to the ring's end. The region stays as it was there, as
    /// Emacs's `goto-char` leaves it, and no mark is pushed. The agent goes to no buffer it may
    /// not be shown (ADR 0094), and the ring stays as it was then.
    ///
    /// # Errors
    ///
    /// What to say: no place, or none for the agent. Nothing changes.
    pub fn pop_global_mark(&mut self) -> Result<(), &'static str> {
        // A place whose buffer went some other way than a kill goes now, as in Emacs.
        while let Some((buffer, _)) = self.global_marks.newest() {
            if self.buffer(buffer).is_some() {
                break;
            }
            self.global_marks.forget(buffer);
        }
        let (buffer, offset) = self.global_marks.newest().ok_or(NO_GLOBAL_MARK)?;
        let shown = self.buffer(buffer).ok_or(NO_GLOBAL_MARK)?;
        if self.runs_for_agent()
            && (shown.is_secret()
                || shown.withheld_from_agents()
                || shown.agent_pending()
                || shown.loading().is_some())
        {
            return Err(NOT_FOR_AGENT);
        }
        let clipped = shown.text().clip_offset(offset, Bias::Left);
        let target = crate::motion::cluster_start(&shown.display(), clipped);
        let index = self.selected;
        if self.windows[index].buffer() != buffer {
            self.show(buffer);
            if self.windows[index].buffer() != buffer {
                return Err(NO_GLOBAL_MARK);
            }
        }
        self.global_marks.rotate();
        let selections = &mut self.windows[index].selections;
        selections.keep_primary();
        selections.move_primary(target);
        self.follow_window(index);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
