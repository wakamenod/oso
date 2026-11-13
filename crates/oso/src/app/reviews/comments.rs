//! A review's comments in the app (ADR 0122, step 3b): which thread carries a file's comments
//! to a new reading, what the view thread is told of each file's comments, files that left
//! the listing, and which agent comments the person has read.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here lays out a card or
//! matches lines: the view thread lays out, the review's own thread carries. Work here per
//! change costs that file's comments, and reading marks cost the rows in view, looked for only
//! after the person's own input moved the view or the text changed.
//!
//! - The model's current reading of a file is the one the review buffer shows: a carry's result
//!   is applied only as the text that holds it goes in.
//! - A file with no comments is carried on the view thread, which only hashes; one with
//!   comments on the review's own thread, which reads old sides, in order, so each result is
//!   the oldest carry asked.

use std::collections::HashSet;
use std::sync::Arc;

use oso_core::editor::ReviewId;
use oso_core::review::{Author, Comment, CommentId, Review, RowKind, Rows};

use super::super::App;
use crate::review::view::{Carrying, Tell};
use crate::review::{Entry, Reading, Request};

impl App {
    /// Tells the view the review's file has a new reading, its comments carried at once when
    /// it has none, and on the review's own thread when it has some.
    pub(super) fn tell_file(&mut self, index: usize, reading: Arc<Reading>) {
        let id = self.reviews.open[index].id;
        let commented = self
            .editor
            .review(id)
            .is_some_and(|model| model.comments_on(&reading.entry.path).next().is_some());
        let carry = match self.carry_for(id, &reading) {
            None => Carrying::Nothing,
            Some(carry) if !commented => Carrying::Here(carry),
            Some(carry) => {
                let running = &mut self.reviews.open[index];
                let asked = running
                    .reviewer
                    .as_ref()
                    .is_some_and(|reviewer| reviewer.ask(Request::Carry(carry)));
                if asked {
                    running.carrying.push_back(Arc::clone(&reading));
                    Carrying::Elsewhere
                } else {
                    // With no thread to read old sides, comments whose side moved go outdated.
                    self.carry_for(id, &reading)
                        .map_or(Carrying::Nothing, Carrying::Here)
                }
            }
        };
        self.reviews.open[index]
            .view
            .tell(Tell::File { reading, carry });
    }

    /// The review's own thread carried the oldest carry asked: the view places the file's
    /// cards by it, and hands it back with the text that shows them.
    pub(super) fn review_carried(
        &mut self,
        index: usize,
        carried: oso_core::review::Carried,
        cut_short: bool,
    ) {
        let running = &mut self.reviews.open[index];
        let Some(reading) = running.carrying.pop_front() else {
            return self.freer.free(carried);
        };
        if cut_short {
            log::info!("a review's comments ran out of time to be carried");
        }
        running.view.tell(Tell::Carried {
            reading,
            carried,
            cut_short,
        });
    }

    /// Carries the file's comments to `reading` again, as applying the last carry asked: on the
    /// view thread for a file with none, on the review's own thread otherwise. The view drops
    /// what comes for a reading that is no longer the file's.
    pub(super) fn carry_again(&mut self, index: usize, reading: Arc<Reading>) {
        let id = self.reviews.open[index].id;
        let commented = self
            .editor
            .review(id)
            .is_some_and(|model| model.comments_on(&reading.entry.path).next().is_some());
        let Some(carry) = self.carry_for(id, &reading) else {
            return;
        };
        let running = &mut self.reviews.open[index];
        if !commented {
            running.view.tell(Tell::Carry { reading, carry });
            return;
        }
        let asked = running
            .reviewer
            .as_ref()
            .is_some_and(|reviewer| reviewer.ask(Request::Carry(carry)));
        if asked {
            running.carrying.push_back(reading);
        } else if let Some(carry) = self.carry_for(id, &reading) {
            self.reviews.open[index]
                .view
                .tell(Tell::Carry { reading, carry });
        }
    }

    /// The review's own thread failed the oldest carry asked, or every carry asked of a thread
    /// that is gone with `all`: each is carried on the view thread instead, so that no file
    /// waits for one, and comments whose old side moved go outdated.
    pub(super) fn carries_failed(&mut self, index: usize, all: bool) {
        let id = self.reviews.open[index].id;
        let failed: Vec<Arc<Reading>> = if all {
            self.reviews.open[index].carrying.drain(..).collect()
        } else {
            self.reviews.open[index]
                .carrying
                .pop_front()
                .into_iter()
                .collect()
        };
        for reading in failed {
            if let Some(carry) = self.carry_for(id, &reading) {
                self.reviews.open[index]
                    .view
                    .tell(Tell::Carry { reading, carry });
            }
        }
    }

    /// Tells the view what the review's file at `path` holds of comments now, and the model's
    /// reading they are placed in.
    pub(super) fn tell_cards(&mut self, index: usize, path: &[u8]) {
        let id = self.reviews.open[index].id;
        let Some(model) = self.editor.review(id) else {
            return;
        };
        // Each clone costs counts: the text and the path are shared.
        let comments: Vec<Comment> = model.comments_on(path).cloned().collect();
        let reading = model.file(path).cloned();
        self.reviews.open[index].view.tell(Tell::Cards {
            path: path.into(),
            comments,
            reading,
        });
    }

    /// The file's comments changed: its section is built again.
    pub(super) fn redraw_comments(&mut self, review: ReviewId, path: &[u8]) {
        if let Some(index) = self.reviews.index(review) {
            self.tell_cards(index, path);
        }
    }

    /// Takes the files that left a whole listing out of the review: their comments go outdated
    /// and stay in view under the listed files (Q-E). A cut listing tells nothing of the files
    /// it left out.
    pub(super) fn forget_unlisted(&mut self, index: usize, entries: &[Entry], whole: bool) {
        if !whole {
            return;
        }
        let id = self.reviews.open[index].id;
        let Some(model) = self.editor.review_mut(id) else {
            return;
        };
        let gone = unlisted(model, entries);
        let mut commented = Vec::new();
        for path in gone {
            model.remove_file(&path);
            if model.comments_on(&path).next().is_some() {
                commented.push(path);
            }
        }
        for path in commented {
            self.tell_cards(index, &path);
        }
    }

    /// The person's own input came: a key, typed text or the wheel, so that what the selected
    /// review window shows after it counts as read.
    pub(in crate::app) fn note_person_acted(&mut self) {
        self.reviews.person_acted = true;
    }

    /// After commands and events: tells each view whether agent cards fold, when that changed,
    /// and marks read the agent comments the selected review window shows whole.
    pub(super) fn sync_review_comments(&mut self) {
        for index in 0..self.reviews.open.len() {
            let id = self.reviews.open[index].id;
            let folded = self.editor.agents_folded(id);
            let running = &mut self.reviews.open[index];
            if running.folded != folded {
                running.folded = folded;
                running.view.tell(Tell::Folded(folded));
            }
        }
        self.mark_shown_read();
    }

    /// Marks read each unread agent comment whose label and every row of its text are in the
    /// selected window, a review's, after the person's own input, and builds its file's section
    /// again so that its label loses `· new` (Q7). None is marked while agent cards fold, or
    /// while the text shown was built with them folded. Costs the rows in view, and nothing when
    /// the view and its text are as when last looked at.
    fn mark_shown_read(&mut self) {
        // What the agent channel brings into view is not read: only the person's input counts.
        if !std::mem::take(&mut self.reviews.person_acted) || self.editor.prompt().is_some() {
            return;
        }
        let (window, shown) = self.editor.tiled();
        let looked = (
            window.id(),
            window.scroll().start,
            window.size(),
            shown.revision(),
        );
        let (window, buffer) = (window.id(), window.buffer());
        let Some(id) = self.editor.review_of(buffer) else {
            return;
        };
        if self.reviews.looked.replace(looked) == Some(looked) {
            return;
        }
        let mut screen = std::mem::take(&mut self.reviews.screen);
        self.editor.screen_of(window, &mut screen);
        let lines = screen.rows().iter().map(|row| row.line);
        let (first, last) = lines.fold((usize::MAX, 0), |(lo, hi), line| {
            (lo.min(line), hi.max(line))
        });
        self.reviews.screen = screen;
        let (Some(rows), Some(model)) = (self.editor.review_rows(id), self.editor.review(id))
        else {
            return;
        };
        let folded = self.editor.agents_folded(id)
            || (self.reviews.index(id)).is_some_and(|index| self.reviews.open[index].shown_folded);
        let (read, mut paths) = read_in_view(rows, model, first, last, folded);
        if read.is_empty() {
            return;
        }
        if let Some(model) = self.editor.review_mut(id) {
            model.mark_read(&read);
        }
        if let Some(index) = self.reviews.index(id) {
            paths.dedup();
            for path in paths {
                self.tell_cards(index, &path);
            }
        }
    }
}

/// The files `model` has a reading of that `entries` do not list.
fn unlisted(model: &Review, entries: &[Entry]) -> Vec<Box<[u8]>> {
    let listed: HashSet<&[u8]> = entries.iter().map(|entry| &*entry.path).collect();
    model
        .files()
        .filter(|(path, _)| !listed.contains(path))
        .map(|(path, _)| path.into())
        .collect()
}

/// The unread agent comments whose label and every text row run within the lines `first` to
/// `last` of `rows`, and the paths of their files, each once in a row. None while agent cards
/// show `folded`, nor one whose text has no row: its label alone shows, past the text's room or
/// in a resolved thread.
fn read_in_view(
    rows: &Rows,
    model: &Review,
    first: usize,
    last: usize,
    folded: bool,
) -> (Vec<CommentId>, Vec<Box<[u8]>>) {
    let mut read = Vec::new();
    let mut paths: Vec<Box<[u8]>> = Vec::new();
    if folded {
        return (read, paths);
    }
    for line in first..=last {
        let Some(row) = rows.at(line) else {
            break;
        };
        let RowKind::Label { comment } = row.kind else {
            continue;
        };
        let Some(shown) = model.comment(comment) else {
            continue;
        };
        if shown.author() != Author::Agent || shown.is_read() {
            continue;
        }
        let is_text =
            |at: usize| (rows.at(at)).is_some_and(|row| row.kind == RowKind::Card { comment });
        if !shown.text().is_empty() && !is_text(line + 1) {
            continue;
        }
        // Its text rows run on from its label, looked at as far as the row past the view: the
        // last must be in view, so that row must not be one of them.
        let mut end = line + 1;
        while end <= last && is_text(end) {
            end += 1;
        }
        if !is_text(end) {
            read.push(comment);
            if paths.last().is_none_or(|path| **path != *row.path) {
                paths.push(row.path.into());
            }
        }
    }
    (read, paths)
}

#[cfg(test)]
#[path = "comments_tests.rs"]
mod tests;
