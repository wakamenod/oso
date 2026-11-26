//! Where a saved review's cursor was (ADR 0122, step 6): taken as a line of a side of a file,
//! or the file's row, and put back once a text shows that file.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! A review buffer never wraps, so a buffer line is a row here, as the review's rows count
//! them.

use oso_core::editor::ReviewId;
use oso_core::review::Rows;
use oso_core::selection::SelectionSet;
use oso_core::window::{Scroll, Window, WindowId};

use crate::app::App;
use crate::review::saved::Place;

/// How many rows above a cursor put back the window shows, where the place saved does not say
/// (a file saved before ADR 0184).
const PLACE_ABOVE: usize = 3;

/// Where a saved place goes in a review's rows now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::reviews) enum Spot {
    /// At this line.
    At(usize),
    /// Not yet: a later text may show it.
    Wait,
    /// Nowhere: every file was read, and its file is not shown.
    Gone,
}

/// Where `place` goes in `rows`, `read_once` once every file of the review was read: its line,
/// or its file's row. A place on a line waits while only its file's row is found and not every
/// file was read, as a text shows a file's row before its lines (ADR 0184). O(log of files,
/// then the file's runs).
pub(in crate::app::reviews) fn spot(rows: &Rows, place: &Place, read_once: bool) -> Spot {
    match rows.line_of(&place.path, place.line) {
        Some(line) if place.line.is_none() || read_once => Spot::At(line),
        // `line_of` gives the file's row when no run of the file has the line or one after it.
        Some(line) if rows.at(line).and_then(|row| row.line()).is_none() => Spot::Wait,
        Some(line) => Spot::At(line),
        None if read_once => Spot::Gone,
        None => Spot::Wait,
    }
}

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
        let text = buffer.text();
        let line = text.offset_to_point(window.selections.primary().cursor).row;
        let top = text.offset_to_point(window.scroll().start).row;
        let row = self.editor.review_rows_of(window.buffer())?.at(line)?;
        Some(Place {
            path: row.path.into(),
            line: row.line(),
            // The rows above it the window shows (ADR 0184).
            above: u32::try_from(line.saturating_sub(top)).ok(),
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
        let read_once = self.reviews.open[index].read_once;
        let spot = (self.editor.review_rows(id)).map(|rows| spot(rows, place, read_once));
        let line = match spot {
            Some(Spot::At(line)) => line,
            Some(Spot::Gone) => {
                self.reviews.open[index].saving.place = None;
                return;
            }
            Some(Spot::Wait) | None => return,
        };
        let above = place.above;
        self.reviews.open[index].saving.place = None;
        let windows: Vec<WindowId> = (self.editor.windows())
            .filter(|window| window.buffer() == buffer && window.selections.primary().cursor == 0)
            .map(Window::id)
            .collect();
        for window in windows {
            self.put_cursor_at(window, buffer, line, above);
        }
    }

    /// The review at `index` was continued from memory into `window` (ADR 0184): its cursor
    /// goes back where the person left it, with the rows above it, wherever the window's
    /// cursor is. O(log of rows).
    pub(in crate::app::reviews) fn put_back_seen(&mut self, index: usize, window: WindowId) {
        let id = self.reviews.open[index].id;
        let Some(place) = self.place_seen(index) else {
            return;
        };
        let Some(buffer) = self.editor.review_buffer(id) else {
            return;
        };
        let shows = self.editor.window_by_id(window).map(Window::buffer) == Some(buffer);
        let line =
            (self.editor.review_rows(id)).and_then(|rows| rows.line_of(&place.path, place.line));
        if let (true, Some(line)) = (shows, line) {
            self.put_cursor_at(window, buffer, line, place.above);
        }
    }

    /// Puts `window`'s cursor at the start of line `line` of the review buffer `buffer`, with
    /// `above` rows above it, else a few; never in a buffer that may hold secrets.
    fn put_cursor_at(
        &mut self,
        window: WindowId,
        buffer: oso_core::buffer::BufferId,
        line: usize,
        above: Option<u32>,
    ) {
        let Some(shown) = (self.editor.buffer(buffer)).filter(|shown| !shown.is_secret()) else {
            return;
        };
        let above = (above.and_then(|above| usize::try_from(above).ok())).unwrap_or(PLACE_ABOVE);
        let text = shown.text();
        let offset = text.line_start(line);
        let scroll = Scroll {
            start: text.line_start(line.saturating_sub(above)),
            left: 0,
        };
        self.editor
            .restore_positions_in(window, SelectionSet::new(offset), scroll);
    }
}

#[cfg(test)]
#[path = "place_tests.rs"]
mod tests;
