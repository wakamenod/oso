//! A click in a window's text moves point there (ADR 0183): the cell under it, read through
//! the rows the frame draws (ADR 0100, ADR 0104), so that wrapped rows, their indent and a
//! window scrolled sideways agree with what the person sees.
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use super::Editor;
use crate::display_map::DisplayPoint;
use crate::selection::MarkActive;
use crate::sum_tree::Bias;
use crate::window::{Screen, WindowId};

impl Editor {
    /// Moves point of the tiled window `id` to the cell at `row`, `column` of its text area,
    /// counted from its top left, as a plain motion: one cursor, the region hidden, no mark
    /// pushed, goal cleared; then follows the cursor (and its review pair). False if `id` is
    /// no tiled window.
    ///
    /// Past the end of a line, point goes to its end; past the end of a wrapped row that its
    /// line goes on from, to the last character the row shows; below the last row, to the end
    /// of the buffer; on a character that takes several cells, as a wide one, a tab or an
    /// escape, to its start.
    pub fn click_point(&mut self, id: WindowId, row: usize, column: usize) -> bool {
        let Some(index) = self.tiled_window_index(id) else {
            return false;
        };
        let mut screen = Screen::default();
        self.screen_of(id, &mut screen);
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let offset = match screen.rows().get(row) {
            None => display.len(),
            Some(shown) => {
                let mut at = screen
                    .origin(shown)
                    .saturating_add(column)
                    .max(shown.column);
                if shown.end_column != usize::MAX {
                    // A row its line goes on from ends before the next row's start.
                    at = at.min(shown.end_column.saturating_sub(1).max(shown.column));
                }
                display.display_point_to_offset(DisplayPoint::new(shown.line, at), Bias::Left)
            }
        };
        let selections = &mut window.selections;
        selections.keep_primary();
        selections.move_primary(offset);
        selections.set_mark_active(MarkActive::Inactive);
        if index == self.selected {
            self.follow_cursor();
        } else {
            self.follow_window(index);
        }
        true
    }
}

#[cfg(test)]
#[path = "click_tests.rs"]
mod tests;
