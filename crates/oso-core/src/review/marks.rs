//! The person's marks on a review's hunks (ADR 0122, "Decisions", steps 4 and 5): decisions,
//! viewed marks and what dropped, the hunks new since the reading before, and how far the person
//! is, counted as the marks change.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use std::sync::Arc;

use super::Review;
use super::decision::{Decision, Mark};
use super::shape::Shape;

/// How many hunks of the current readings hold a decision, a viewed mark, and in all.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Progress {
    decided: usize,
    viewed: usize,
    hunks: usize,
}

impl Progress {
    /// Counts `marks` in, or out with `out`.
    pub(super) fn count(&mut self, marks: &[Mark], out: bool) {
        let decided = marks.iter().filter(|mark| mark.decision.is_some()).count();
        let viewed = marks.iter().filter(|mark| mark.viewed).count();
        let step = |total: &mut usize, by: usize| {
            *total = if out {
                total.saturating_sub(by)
            } else {
                *total + by
            };
        };
        step(&mut self.decided, decided);
        step(&mut self.viewed, viewed);
        step(&mut self.hunks, marks.len());
    }
}

impl Review {
    /// Records, or clears with `None`, the decision on hunk `hunk` of `path`; `false` when the
    /// current reading has no such hunk.
    pub fn decide(&mut self, path: &[u8], hunk: usize, decision: Option<Decision>) -> bool {
        self.change_mark(path, hunk, |mark| mark.decision = decision)
    }

    /// The decision on hunk `hunk` of `path`.
    pub fn decision(&self, path: &[u8], hunk: usize) -> Option<Decision> {
        self.mark(path, hunk)?.decision
    }

    /// Marks hunk `hunk` of `path` viewed, or not; `false` when there is no such hunk.
    pub fn set_viewed(&mut self, path: &[u8], hunk: usize, viewed: bool) -> bool {
        self.change_mark(path, hunk, |mark| mark.viewed = viewed)
    }

    /// Whether hunk `hunk` of `path` is marked viewed.
    pub fn viewed(&self, path: &[u8], hunk: usize) -> bool {
        self.mark(path, hunk).is_some_and(|mark| mark.viewed)
    }

    /// How many decisions and viewed marks on `path` dropped because their hunks changed or
    /// the file left the review, since [`Review::clear_dropped`].
    pub fn dropped(&self, path: &[u8]) -> usize {
        self.files.get(path).map_or(0, |state| state.dropped)
    }

    /// Whether hunk `hunk` of `path` is new since the file's reading before and the person has
    /// not looked at it yet (ADR 0122, step 4).
    pub fn changed(&self, path: &[u8], hunk: usize) -> bool {
        self.mark(path, hunk).is_some_and(|mark| mark.changed)
    }

    /// The person looked at hunk `hunk` of `path`: it is no longer said to be changed. Returns
    /// whether it was.
    pub fn seen(&mut self, path: &[u8], hunk: usize) -> bool {
        if !self.changed(path, hunk) {
            return false;
        }
        self.change_mark(path, hunk, |mark| mark.changed = false)
    }

    /// The readings let go since the last call, for the host to free off the UI thread, as a
    /// large one's hunks take time to drop.
    pub fn take_released(&mut self) -> Vec<Arc<Shape>> {
        std::mem::take(&mut self.released)
    }

    /// How far the person is through the review: the hunks of every current reading that hold a
    /// decision, those marked viewed, and all of them. O(1): counted as marks change.
    pub fn progress(&self) -> (usize, usize, usize) {
        let Progress {
            decided,
            viewed,
            hunks,
        } = self.progress;
        (decided, viewed, hunks)
    }

    /// Forgets the dropped count of `path`, once the person looked.
    pub fn clear_dropped(&mut self, path: &[u8]) {
        if let Some(state) = self.files.get_mut(path) {
            state.dropped = 0;
        }
    }
}
