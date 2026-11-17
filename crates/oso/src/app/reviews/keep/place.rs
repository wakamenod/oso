//! Where a saved review's cursor was (ADR 0122, step 6): taken as a line of a side of a file,
//! or the file's row, and put back once a text shows that file.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! A review buffer never wraps, so a buffer line is a row here, as the review's rows count
//! them.

use oso_core::editor::ReviewId;
use oso_core::selection::SelectionSet;
use oso_core::window::{Scroll, Window, WindowId};

use crate::app::App;
use crate::review::saved::Place;

/// How many rows above a cursor put back the window shows.
const PLACE_ABOVE: usize = 3;

impl App {
    /// Where the cursor of the window showing the review `id` is, the selected one first: a
    /// line of a side of a file, or the file's row. None from a text that may hold secrets.
    pub(in crate::app::reviews) fn review_place(&self, id: ReviewId) -> Option<Place> {
        let shows = |buffer| self.editor.review_of(buffer) == Some(id);
        let (selected, _) = self.editor.tiled();
        let window = std::iter::once(selected)
            .chain(self.editor.windows())
            .find(|window| shows(window.buffer()))?;
        let buffer = self.editor.buffer(window.buffer())?;
        if buffer.is_secret() {
            return None;
        }
        let line = buffer
            .text()
            .offset_to_point(window.selections.primary().cursor)
            .row;
        let row = self.editor.review_rows_of(window.buffer())?.at(line)?;
        Some(Place {
            path: row.path.into(),
            line: row.line(),
        })
    }

    /// A text of the review at `index` went in: the cursor saved goes back to its place, once
    /// the text shows its file, in each window showing the review whose cursor is still at the
    /// top, as the person did not move it yet. Once every file was read, a place not shown is
    /// let go. O(1) for the file, then its runs.
    pub(in crate::app::reviews) fn put_back_place(&mut self, index: usize) {
        let id = self.reviews.open[index].id;
        let Some(place) = &self.reviews.open[index].saving.place else {
            return;
        };
        let Some(buffer) = self.editor.review_buffer(id) else {
            return;
        };
        let line =
            (self.editor.review_rows(id)).and_then(|rows| rows.line_of(&place.path, place.line));
        let Some(line) = line else {
            if self.reviews.open[index].read_once {
                self.reviews.open[index].saving.place = None;
            }
            return;
        };
        self.reviews.open[index].saving.place = None;
        let Some(shown) = self
            .editor
            .buffer(buffer)
            .filter(|shown| !shown.is_secret())
        else {
            return;
        };
        let text = shown.text();
        let offset = text.line_start(line);
        let scroll = Scroll {
            start: text.line_start(line.saturating_sub(PLACE_ABOVE)),
            left: 0,
        };
        let windows: Vec<WindowId> = (self.editor.windows())
            .filter(|window| window.buffer() == buffer && window.selections.primary().cursor == 0)
            .map(Window::id)
            .collect();
        for window in windows {
            self.editor
                .restore_positions_in(window, SelectionSet::new(offset), scroll);
        }
    }
}
