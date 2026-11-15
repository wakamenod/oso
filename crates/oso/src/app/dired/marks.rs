//! A listing's marks (ADR 0134): the names marked, kept by the app for each listing, and shown
//! as a `*` in each marked row's first column.
//!
//! Must not depend on: key presses or how a frame is drawn. `m`, `u` and `DEL` change one
//! character a row on the UI thread; `U` and `t`, which may change every row, have the lister's
//! thread build the listing again, so no pass over the rows runs here.

use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::editor::RowMark;

use super::App;

impl App {
    /// Marks the rows at `lines` of `buffer`, or unmarks them; `.` and `..` are never marked.
    pub(super) fn mark_listed_rows(&mut self, buffer: BufferId, lines: &[usize], on: bool) {
        let Some(listing) = self.dired.by_buffer.get_mut(&buffer) else {
            return;
        };
        if listing.rows_of != listing.directory {
            return self.editor.message("Still listing the directory");
        }
        let mut rows = Vec::new();
        let mut delta: isize = 0;
        for &line in lines {
            let Some(row) = listing.row_at(line) else {
                continue;
            };
            if row.name == "." || row.name == ".." {
                continue;
            }
            let (name, face, name_len) = (row.name.clone(), row.face, row.name_len);
            let marked = Arc::make_mut(&mut listing.marked);
            let changed = if on {
                marked.insert(name)
            } else {
                marked.remove(&name)
            };
            if changed {
                delta += if on { 1 } else { -1 };
                rows.push(RowMark {
                    line,
                    marked: on,
                    face,
                    name_len,
                });
            }
        }
        if rows.is_empty() {
            return;
        }
        let before = self
            .editor
            .listing(buffer)
            .map_or(0, |record| record.marked);
        let now = before.saturating_add_signed(delta);
        self.editor.set_listing_marks(buffer, &rows, now);
        self.request_redraw();
    }

    /// Unmarks every row of `buffer`, listing it again with no marks.
    pub(super) fn unmark_all(&mut self, buffer: BufferId) {
        let Some(listing) = self.dired.by_buffer.get_mut(&buffer) else {
            return;
        };
        listing.marked = Arc::default();
        listing.invert = false;
        self.list_again(buffer);
    }

    /// Marks every row of `buffer` not marked and unmarks those that are, as the lister's thread
    /// lists it again.
    pub(super) fn toggle_marks(&mut self, buffer: BufferId) {
        let Some(listing) = self.dired.by_buffer.get_mut(&buffer) else {
            return;
        };
        // Twice is no change.
        listing.invert = !listing.invert;
        self.list_again(buffer);
    }
}
