//! A saved review read back (ADR 0122, step 6): its model goes in the review just opened, with
//! the view's choices, its cursor's place and the layout it kept (ADR 0184) to put back.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here reads a file: the
//! store's thread read it.

use std::collections::HashSet;

use oso_core::review::Review;

use super::super::store::Found;
use super::Choices;
use crate::app::App;

impl App {
    /// Puts `found`'s model in the review at `index`, which holds nothing yet, with the view's
    /// choices it kept.
    pub(super) fn restore_review(&mut self, index: usize, found: Found) {
        let id = self.reviews.open[index].id;
        let Found {
            review,
            view,
            left_out,
            pull,
            checkpoint,
            left,
            space,
        } = found;
        // A pull request's review: the person's last review of it (step 2).
        if checkpoint.is_some() {
            self.reviews.open[index].checkpoint = checkpoint;
        }
        // A commit's review stepped to from a pull request: linked again once that pull
        // request's commits are listed (ADR 0167).
        if pull.is_some() {
            self.want_pull(index, pull);
        }
        let comments = review.comments().count();
        let Some(model) = self.editor.review_mut(id) else {
            return self.freer.free(review);
        };
        let empty = std::mem::replace(model, review);
        self.freer.free(empty);
        // The view draws a file's cards from what it is told of its comments: each file's once,
        // as its first reading comes, or once a listing says it left (`comments`).
        let untold: HashSet<Box<[u8]>> = (self.editor.review(id).into_iter())
            .flat_map(Review::comments)
            .map(|comment| Box::from(comment.path()))
            .collect();
        self.reviews.open[index].saving.untold = untold;
        self.editor.choose_review_layout(id, view.choice);
        self.editor.fold_agents(id, view.folded);
        self.editor.set_review_filter(id, view.filter.clone());
        if view.files {
            self.show_files_again(id);
        }
        let checkpoint = self.reviews.open[index].checkpoint_moves;
        // Nothing to save until it changes.
        let revision = self.editor.review(id).map_or(0, Review::revision);
        let saving = &mut self.reviews.open[index].saving;
        saving.place_handed.clone_from(&view.place);
        saving.place = view.place;
        // The layout's state, put back once a text shows if it was continued (ADR 0184).
        if view.layout.is_some() {
            saving.layout = view.layout;
            saving.layout_read = true;
        }
        let choices = Choices {
            choice: view.choice,
            folded: view.folded,
            filter: view.filter,
            files: view.files,
            checkpoint,
        };
        saving.handed = Some((revision, choices));
        // When it was left before, unless it was left since it opened (ADR 0184).
        if let Some(left) = left.filter(|left| *left > saving.left) {
            saving.left = left;
            saving.space = space;
            saving.left_handed = left;
        }
        let noun = if comments == 1 { "comment" } else { "comments" };
        if left_out > 0 {
            log::info!("{left_out} lines of a saved review were left out");
            self.editor.message(format!(
                "{comments} saved {noun} brought back; {left_out} lines of the saved review \
                 could not be read and are left out"
            ));
        } else if comments > 0 {
            self.editor
                .message(format!("{comments} saved {noun} brought back"));
        }
    }
}
