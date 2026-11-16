//! A window's history of jumps (ADR 0138, Go to definition and back, step 8): each jump of
//! `M-.`, or from a float of places, keeps the place it left, and `M-,` goes back through them,
//! `C-M-,` forward again, as Emacs 29's `xref-go-back` and `xref-go-forward` with a history for
//! each window. Not ADR 0088's places, which keep where the window was in each buffer.
//!
//! Must not depend on: `editor` or `command`.
//!
//! - It holds [`JUMPS_MAX`] places back; past that, the oldest goes. A new jump empties the way
//!   forward.
//! - A place is a buffer and an offset, which follows the edits a window's cursors follow
//!   ([`Jumps::follow`]); an edit the history does not see, such as a sliced undo's, leaves the
//!   offset as it was, and going there clips it to the text.
//! - A killed buffer's places go.
//! - `Debug` gives counts only: an offset tells how long words are in a buffer that may hold
//!   secrets.

use std::collections::VecDeque;
use std::fmt;

use super::Window;
use crate::buffer::BufferId;
use crate::selection::{OffsetEdit, follow_offset};
use crate::sum_tree::Bias;

/// The most places a window's history keeps back.
pub const JUMPS_MAX: usize = 64;

/// A window's places back and forward, the nearest last in each.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Jumps {
    back: VecDeque<(BufferId, usize)>,
    forward: Vec<(BufferId, usize)>,
}

impl fmt::Debug for Jumps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jumps")
            .field("back", &self.back.len())
            .field("forward", &self.forward.len())
            .finish()
    }
}

impl Jumps {
    /// Keeps `left`, the place a jump leaves, as the nearest back, and empties the way forward.
    pub(crate) fn push(&mut self, left: (BufferId, usize)) {
        self.forward.clear();
        self.keep_back(left);
    }

    fn keep_back(&mut self, place: (BufferId, usize)) {
        if self.back.len() == JUMPS_MAX {
            self.back.pop_front();
        }
        self.back.push_back(place);
    }

    /// The nearest place back, `here` kept as the nearest forward, or `None` if there is none.
    pub(crate) fn back(&mut self, here: (BufferId, usize)) -> Option<(BufferId, usize)> {
        let place = self.back.pop_back()?;
        self.forward.push(here);
        Some(place)
    }

    /// The nearest place forward, `here` kept as the nearest back, or `None` if there is none.
    pub(crate) fn forward(&mut self, here: (BufferId, usize)) -> Option<(BufferId, usize)> {
        let place = self.forward.pop()?;
        self.keep_back(here);
        Some(place)
    }

    /// Lets the places in `buffer` go, as it is killed.
    pub(crate) fn forget(&mut self, buffer: BufferId) {
        self.back.retain(|(kept, _)| *kept != buffer);
        self.forward.retain(|(kept, _)| *kept != buffer);
    }

    /// Moves the places in `buffer` through `steps`, each a step's edits, as an Emacs marker:
    /// a place at an insertion stays before it.
    pub(crate) fn follow(&mut self, buffer: BufferId, steps: &[Vec<OffsetEdit>]) {
        let places = self.back.iter_mut().chain(self.forward.iter_mut());
        for (kept, offset) in places {
            if *kept == buffer {
                for edits in steps {
                    *offset = follow_offset(*offset, edits, Bias::Left);
                }
            }
        }
    }

    /// How many places are back and forward.
    pub fn len(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }

    /// Whether it holds no place.
    pub fn is_empty(&self) -> bool {
        self.back.is_empty() && self.forward.is_empty()
    }
}

impl Window {
    /// Its history of jumps.
    pub fn jumps(&self) -> &Jumps {
        &self.jumps
    }

    pub(crate) fn jumps_mut(&mut self) -> &mut Jumps {
        &mut self.jumps
    }
}

#[cfg(test)]
mod tests;
