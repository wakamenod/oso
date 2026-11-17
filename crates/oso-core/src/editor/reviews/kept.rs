//! What the host keeps of a review across a restart (ADR 0122, step 6): whether it saves the
//! review as it changes, so that the person's comments are not lost with it, and the model of
//! a review closed, which the host takes to save once more and to free off the UI thread.
//!
//! Must not depend on: the host's store, files or threads.

use super::{Editor, ReviewId};
use crate::review::Review;

impl Editor {
    /// The model of the review `id`, closed since: once, for the host to save what it held and
    /// free it off the UI thread (ADR 0122, step 6).
    pub fn take_closed_review(&mut self, id: ReviewId) -> Option<Review> {
        let index = (self.reviews.closed.iter()).position(|(closed, _)| *closed == id)?;
        Some(self.reviews.closed.swap_remove(index).1)
    }

    /// Whether the host keeps the review `id` across a restart, as it saves it as it changes:
    /// its comments are then not lost when it closes or the editor goes (step 6).
    pub fn review_kept(&self, id: ReviewId) -> bool {
        self.reviews.get(id).is_some_and(|open| open.kept)
    }

    /// The host keeps the review `id` across a restart, or no longer can, as a write failed.
    pub fn set_review_kept(&mut self, id: ReviewId, kept: bool) {
        if let Some(open) = self.reviews.get_mut(id) {
            open.kept = kept;
        }
    }
}
