//! The rows of a request's body that a window shows, from what is laid out of it (ADR 0182,
//! "Reading").
//!
//! Must not depend on: the app or the clock; it reads a screen and the display it was laid out of.

use std::ops::Range;

use oso_core::display_map::DisplaySnapshot;
use oso_core::window::Screen;

use super::Row;

/// Puts the rows of `body`, its bytes in the buffer `display` is of, that `screen` shows in its
/// first `height` rows in `out`, in place of what it held.
///
/// A row's cells are counted to where its text ends, past the window's edge too, and not to what is
/// drawn of it: where the next row of its line starts, or the line ends.
pub fn rows(
    screen: &Screen,
    display: &DisplaySnapshot,
    height: usize,
    body: &Range<usize>,
    out: &mut Vec<Row>,
) {
    out.clear();
    for (at, row) in screen.rows().iter().take(height).enumerate() {
        if !body.contains(&row.start) {
            continue;
        }
        let end = if row.continues {
            display.offset_to_display_point(row.end).column
        } else {
            display.row_cells(row.line)
        };
        out.push(Row {
            start: row.start - body.start,
            end: row.end.min(body.end) - body.start,
            at,
            cells: end.saturating_sub(screen.origin(row)),
        });
    }
}
