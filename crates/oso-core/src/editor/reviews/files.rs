//! A review's files pane and its filter (ADR 0122, step 6b): a narrow window at the left of a
//! review's area that lists its files, one row each, and the words that say which files the
//! review shows.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host builds the
//! pane's text on the review's view thread while a window shows it, and tells the view the
//! filter.
//!
//! - The pane is made when a window first shows it, at the left of a window showing the review
//!   or of the whole pair of its panes, and goes, its columns given back to the area at its
//!   right, when it is hidden, after a command once no window shows it, or once no window shows
//!   its review.
//! - Each of its rows is a file's, a section of the pane's rows holding the file's header
//!   alone, under a title: a text put in keeps each window's row by its file's path.
//! - After a command, the pane's cursor follows the file of the review's cursor, when the
//!   selected window shows the review, never from the pane, and never while the review or the
//!   pane may hold secrets (ADR 0044).

use crate::buffer::{BufferId, Mode};
use crate::editor::{Editor, NoPair, ReviewId, ReviewLeftover};
use crate::frame::TooSmall;
use crate::load::{Change, Reloaded};
use crate::motion;
use crate::review::{Filter, Rows};
use crate::syntax::Highlights;
use crate::window::WindowId;

/// The columns the pane takes when it is made, its divider included.
const COLUMNS: usize = 40;

/// What a review keeps of its files pane and its filter.
#[derive(Debug, Default)]
pub(in crate::editor) struct Files {
    pane: Option<Pane>,
    filter: Option<Filter>,
    /// The file whose row the pane's cursor was last put on.
    followed: Option<Box<[u8]>>,
}

/// The pane's buffer, what each of its lines is, and whether it holds no text from the host
/// yet, which it then takes whole.
#[derive(Debug)]
struct Pane {
    buffer: BufferId,
    rows: Rows,
    fresh: bool,
}

impl Editor {
    /// The words that say which files the review `id` shows, if any.
    pub fn review_filter(&self, id: ReviewId) -> Option<&Filter> {
        self.reviews.get(id)?.files.filter.as_ref()
    }

    /// Shows only the files `filter` matches in the review `id`, or every file with `None`.
    /// The host tells the review's view, which lays its text out again.
    pub fn set_review_filter(&mut self, id: ReviewId, filter: Option<Filter>) {
        if let Some(open) = self.reviews.get_mut(id) {
            open.files.filter = filter;
        }
    }

    /// The buffer of the review `id`'s files pane, while it has one.
    pub fn review_files(&self, id: ReviewId) -> Option<BufferId> {
        Some(self.reviews.get(id)?.files.pane.as_ref()?.buffer)
    }

    /// What each line of the review `id`'s files pane is: its title, then a file's header a
    /// row, by the file's path.
    pub fn review_files_rows(&self, id: ReviewId) -> Option<&Rows> {
        self.files_rows(id)
    }

    /// Puts in the review `id`'s files pane's whole new text, with what each line is and its
    /// highlights, at O(1) as [`Editor::set_review_text`] does, each window keeping its row by
    /// its file's path. The host calls it only while [`Editor::review_waits`] is false, and
    /// frees what this returns off the UI thread: all it was given while the review has no
    /// pane.
    pub fn set_review_files(
        &mut self,
        id: ReviewId,
        text: Reloaded,
        rows: Rows,
        highlights: Highlights,
    ) -> ReviewLeftover {
        let Some(pane) = self
            .reviews
            .get(id)
            .and_then(|open| open.files.pane.as_ref())
        else {
            return ReviewLeftover::of(rows, highlights, Some(text));
        };
        let (buffer, fresh) = (pane.buffer, pane.fresh);
        // Each window's file, to keep its row by.
        let kept: Vec<(usize, Option<Box<[u8]>>)> = (0..self.windows.len())
            .filter(|index| self.windows[*index].buffer() == buffer)
            .map(|index| {
                let line = self.window_line(index);
                (index, pane.rows.at(line).map(|row| Box::from(row.path)))
            })
            .collect();
        let mut text = text;
        if fresh {
            text.change = Some(Change {
                start: 0,
                old_end: 0,
                new_end: text.snapshot.text.len(),
            });
        }
        let mut left = self.put_generated_text(buffer, text, highlights);
        left.spare = self.take_spare_rows(id);
        if let Some(pane) = self
            .reviews
            .get_mut(id)
            .and_then(|open| open.files.pane.as_mut())
        {
            left.spare.push(std::mem::replace(&mut pane.rows, rows));
            pane.fresh = false;
        }
        for (index, path) in kept {
            let line = path.and_then(|path| self.files_rows(id)?.line_of(&path, None));
            if let Some(line) = line {
                self.put_cursor_on_line(index, line);
            }
        }
        self.follow_buffer_windows(buffer);
        left
    }

    /// Shows the review `id`'s files pane in a new window at the left of `window`, which shows
    /// the review or a pane of it, or of the whole pair `window` is in, [`COLUMNS`] wide or as
    /// many as leave the area its smallest; or the window that shows it already. Returns the
    /// pane's window, which it does not select. Ends isearch.
    ///
    /// # Errors
    ///
    /// [`NoPair::NoBuffer`] for a window that shows no buffer of the review, and
    /// [`NoPair::TooSmall`] where there is no room. Nothing changes.
    pub fn show_review_files(
        &mut self,
        id: ReviewId,
        window: WindowId,
    ) -> Result<WindowId, NoPair> {
        let index = self.window_index(window).ok_or(NoPair::NoBuffer)?;
        let shows = self.reviews.holding(self.windows[index].buffer());
        if shows.is_none_or(|open| open.id != id) {
            return Err(NoPair::NoBuffer);
        }
        if let Some(buffer) = self.review_files(id)
            && let Some(shown) = self.windows.iter().find(|window| window.buffer() == buffer)
        {
            return Ok(shown.id());
        }
        let new = WindowId::new(self.next_window);
        self.set_split_gutter();
        let mut splits = self.splits.clone();
        splits
            .split_left(window, new, COLUMNS, &self.frame)
            .map_err(|TooSmall| NoPair::TooSmall)?;
        self.isearch_exit();
        self.splits = splits;
        self.next_window += 1;
        self.add_split_off(index, new);
        self.lay_out(self.frame.size());
        // Only now: laying out ends with dropping a pane no window shows.
        let buffer = match self.review_files(id) {
            Some(buffer) => buffer,
            None => self.make_review_files(id),
        };
        if let Some(index) = self.window_index(new) {
            self.window_shows(index, buffer);
        }
        // Its cursor goes to the file of the review's.
        if let Some(files) = self.reviews.get_mut(id) {
            files.files.followed = None;
        }
        self.follow_review_file(window);
        Ok(new)
    }

    /// Hides the review `id`'s files pane: each window showing it goes, its columns given
    /// back to the area at its right, or, as the frame's only window, shows the review again;
    /// and its buffer goes. Returns whether it did: not beneath a prompt (ADR 0072).
    pub fn hide_review_files(&mut self, id: ReviewId) -> bool {
        let Some(buffer) = self.review_files(id) else {
            return false;
        };
        if self.prompt().is_some() {
            return false;
        }
        let shown: Vec<WindowId> = (self.windows.iter())
            .filter(|window| window.buffer() == buffer)
            .map(|window| window.id())
            .collect();
        if !shown.is_empty() {
            self.isearch_exit();
        }
        for window in shown {
            let mut splits = self.splits.clone();
            if splits.remove_toward_next(window, &self.frame) {
                self.splits = splits;
                self.drop_window(window);
                self.lay_out(self.frame.size());
            } else if let (Some(index), Some(review)) =
                (self.window_index(window), self.review_buffer(id))
            {
                self.window_shows(index, review);
            }
        }
        self.drop_review_files(id);
        true
    }

    /// The review `id` closes: its files pane goes first, its columns back to the area beside
    /// it, and its buffer even beneath a prompt.
    pub(in crate::editor) fn close_review_files(&mut self, id: ReviewId) {
        self.hide_review_files(id);
        self.drop_review_files(id);
    }

    /// Makes the review `id`'s files pane's buffer, empty, named after its review buffer.
    fn make_review_files(&mut self, id: ReviewId) -> BufferId {
        let name = (self.review_buffer(id))
            .and_then(|buffer| self.buffer(buffer))
            .map(|buffer| buffer.name().to_owned())
            .unwrap_or_default();
        let base = name.strip_prefix("*review: ").unwrap_or(&name);
        let buffer = self.add_internal_buffer(format!("*review files: {base}"), Mode::ReviewFiles);
        if let Some(shown) = self.buffer_mut(buffer) {
            shown.set_wraps(false);
        }
        if let Some(open) = self.reviews.get_mut(id) {
            open.files.pane = Some(Pane {
                buffer,
                rows: Rows::default(),
                fresh: true,
            });
        }
        buffer
    }

    /// Lets go of the review `id`'s files pane's buffer, its rows going with the review's next
    /// leftover, freed off the UI thread.
    fn drop_review_files(&mut self, id: ReviewId) {
        let Some(open) = self.reviews.get_mut(id) else {
            return;
        };
        let Some(pane) = open.files.pane.take() else {
            return;
        };
        open.spare.push(pane.rows);
        open.files.followed = None;
        self.drop_internal_buffer(pane.buffer);
    }

    /// The rows of the review `id`'s files pane.
    fn files_rows(&self, id: ReviewId) -> Option<&Rows> {
        Some(&self.reviews.get(id)?.files.pane.as_ref()?.rows)
    }

    /// The line the primary cursor of the window at `index` is on.
    fn window_line(&self, index: usize) -> usize {
        let window = &self.windows[index];
        self.buffer(window.buffer()).map_or(0, |buffer| {
            buffer
                .text()
                .offset_to_point(window.selections.primary().cursor)
                .row
        })
    }

    /// Puts the primary cursor of the window at `index` alone at the start of `line`.
    fn put_cursor_on_line(&mut self, index: usize, line: usize) {
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let offset = motion::line_start(&display, line.min(display.row_count().saturating_sub(1)));
        window.selections.keep_primary();
        window.selections.update(|selection| {
            selection.cursor = offset;
            selection.mark = None;
            selection.goal = None;
            Ok::<(), ()>(())
        });
    }

    /// After a command: drops the files panes no window shows, hides those whose review no
    /// window shows, and has each pane's cursor follow its review's file.
    pub(in crate::editor) fn sync_review_files(&mut self) {
        let panes: Vec<(ReviewId, BufferId)> = (self.reviews.open.iter())
            .filter_map(|open| Some((open.id, open.files.pane.as_ref()?.buffer)))
            .collect();
        for (id, buffer) in panes {
            let shown =
                |buffer: BufferId| self.windows.iter().any(|window| window.buffer() == buffer);
            let review_shown = (self.reviews.get(id)).is_some_and(|open| {
                shown(open.buffer)
                    || (open.panes.iter()).any(|panes| panes.buffers.iter().any(|b| shown(*b)))
            });
            if !shown(buffer) {
                self.drop_review_files(id);
            } else if !review_shown {
                self.hide_review_files(id);
            }
        }
        let selected = self.windows[self.selected].id();
        self.follow_review_file(selected);
    }

    /// Puts the cursor of each window showing the files pane of the review `window` shows on
    /// the row of the file of `window`'s cursor, if it moved to another file since.
    fn follow_review_file(&mut self, window: WindowId) {
        let Some(index) = self.window_index(window) else {
            return;
        };
        let buffer = self.windows[index].buffer();
        let Some(id) = self.reviews.holding(buffer).map(|open| open.id) else {
            return;
        };
        let Some(pane) = self.review_files(id) else {
            return;
        };
        // No position leaves, or enters, a buffer that may hold secrets (ADR 0044).
        let secret = self.buffer(pane).is_none_or(|shown| shown.is_secret());
        if secret || self.review_holds_secret(buffer) {
            return;
        }
        let line = self.window_line(index);
        let Some(path) = (self.review_rows_of(buffer))
            .and_then(|rows| rows.at(line))
            .map(|row| Box::<[u8]>::from(row.path))
        else {
            return;
        };
        let followed = self
            .reviews
            .get(id)
            .and_then(|open| open.files.followed.as_deref());
        if path.is_empty() || followed == Some(&*path) {
            return;
        }
        let Some(row) = self
            .files_rows(id)
            .and_then(|rows| rows.line_of(&path, None))
        else {
            return;
        };
        let shown: Vec<usize> = (0..self.windows.len())
            .filter(|index| self.windows[*index].buffer() == pane)
            .collect();
        for index in shown {
            self.put_cursor_on_line(index, row);
            self.follow_window(index);
        }
        if let Some(open) = self.reviews.get_mut(id) {
            open.files.followed = Some(path);
        }
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
