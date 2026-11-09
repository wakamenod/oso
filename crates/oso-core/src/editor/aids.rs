//! The visual aids' switches, and what they fill in the tiled window: trailing whitespace and
//! the current line's band (ADR 0097).
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use std::ops::Range;

use super::Editor;
use crate::frame::{self, CellRect};
use crate::rope::Rope;

/// The most bytes one row's walk back over its trailing whitespace reads: 512 characters
/// of at most three bytes, as U+3000 takes.
const ROW_BYTES: usize = 512 * 3;

/// The most bytes a frame's walks read in all.
const FRAME_BYTES: usize = 16_384 * 3;

/// Which visual aids are on (ADR 0097). Every window reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualAids {
    /// Tabs, full-width and other non-ASCII spaces marked, and trailing whitespace filled.
    pub whitespace: bool,
    /// A band on the primary cursor's row.
    pub current_line: bool,
    /// A band that fades after a large jump.
    pub flash: bool,
    /// Line numbers in a gutter.
    pub line_numbers: bool,
}

impl Default for VisualAids {
    /// ADR 0026's defaults: all on but line numbers.
    fn default() -> Self {
        Self {
            whitespace: true,
            current_line: true,
            flash: true,
            line_numbers: false,
        }
    }
}

impl Editor {
    /// Which visual aids are on.
    pub fn visual_aids(&self) -> VisualAids {
        self.aids
    }

    /// Turns the visual aids on and off as `aids` says.
    pub fn set_visual_aids(&mut self, aids: VisualAids) {
        self.aids = aids;
    }

    /// The cells of the tiled window's trailing whitespace in view, into `out`, which is
    /// emptied first: none with whitespace off, and none on a line with a cursor, so that
    /// typing a space at a line's end does not flash a fill. A row whose trailing run does
    /// not end within its share of the walk gets none, so a long line costs no more than a
    /// short one.
    pub fn trailing_cells(&self, out: &mut Vec<CellRect>) {
        out.clear();
        if !self.aids.whitespace {
            return;
        }
        let (window, buffer) = self.tiled();
        let display = buffer.display();
        let (text, scroll) = (self.frame.window.text, window.scroll());
        let rows = scroll.top..(scroll.top + text.rows).min(display.row_count());
        let cursor_rows: Vec<usize> = window
            .selections
            .iter()
            .map(|selection| display.offset_to_display_point(selection.cursor).row)
            .filter(|row| rows.contains(row))
            .collect();
        let mut left = FRAME_BYTES;
        let (mut ranges, mut tail) = (Vec::new(), Vec::with_capacity(ROW_BYTES));
        for row in rows {
            if cursor_rows.contains(&row) {
                continue;
            }
            if let Some(range) = trailing(buffer.text(), row, &mut left, &mut tail) {
                ranges.push(range);
            }
        }
        frame::match_cells(text, scroll, &display, &ranges, out);
    }

    /// The cells of the band on the tiled window's primary cursor's row, if the current line
    /// is shown and that row is in view.
    pub fn current_line_cells(&self) -> Option<CellRect> {
        if !self.aids.current_line {
            return None;
        }
        let (window, buffer) = self.tiled();
        let (text, scroll) = (self.frame.window.text, window.scroll());
        let row = window.cursor_point(&buffer.display()).row;
        let shown = row.checked_sub(scroll.top).filter(|&row| row < text.rows)?;
        Some(CellRect {
            row: text.row + shown,
            rows: 1,
            ..text
        })
    }
}

/// The trailing whitespace of line `row` in `text`: spaces, tabs and U+3000 before its break
/// or the end of the text. `None` if there is none, or if it does not end within what is
/// `left` of the frame's walk, which this takes from. `tail` is room to read into.
fn trailing(text: &Rope, row: usize, left: &mut usize, tail: &mut Vec<u8>) -> Option<Range<usize>> {
    let (start, end) = (text.line_start(row), text.line_end(row));
    let budget = ROW_BYTES.min(*left);
    let from = end.saturating_sub(budget).max(start);
    *left -= end - from;
    tail.clear();
    for chunk in text.chunks_in_range(from..end) {
        tail.extend_from_slice(chunk.as_bytes());
    }
    let mut at = tail.len();
    loop {
        match tail[..at] {
            [.., b' ' | b'\t'] => at -= 1,
            [.., 0xe3, 0x80, 0x80] => at -= 3,
            _ => break,
        }
    }
    let ended = at > 0 || from == start;
    (ended && at < tail.len()).then(|| from + at..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::selection::SelectionSet;
    use crate::window::ViewSize;

    fn editor(text: &str, cursor: usize) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor.current_mut().0.selections = SelectionSet::new(cursor);
        editor
    }

    fn rows(cells: &[CellRect]) -> Vec<(usize, usize, usize)> {
        cells
            .iter()
            .map(|cell| (cell.row, cell.column, cell.columns))
            .collect()
    }

    #[test]
    fn trailing_whitespace_is_filled_but_on_a_line_with_a_cursor() {
        let mut editor = editor("a  \nb\t\nc\u{3000}\r\nd e\n  \n", 0);
        let mut cells = Vec::new();
        editor.trailing_cells(&mut cells);
        // The cursor's line keeps its two spaces unmarked; the tab, the full-width space
        // before a `\r\n`, and a line of nothing but spaces are filled.
        assert_eq!(rows(&cells), [(1, 1, 7), (2, 1, 2), (4, 0, 2)]);
        editor.set_visual_aids(VisualAids {
            whitespace: false,
            ..VisualAids::default()
        });
        editor.trailing_cells(&mut cells);
        assert!(cells.is_empty());
    }

    #[test]
    fn a_trailing_run_longer_than_the_walk_is_left_unfilled() {
        let long = format!("x{}\nshort  \n", " ".repeat(ROW_BYTES + 10));
        let editor = editor(&long, long.len());
        let mut cells = Vec::new();
        editor.trailing_cells(&mut cells);
        assert_eq!(
            rows(&cells),
            [(1, 5, 2)],
            "the long run is not reached in time"
        );
        let mut left = FRAME_BYTES;
        let text = Rope::from(long.as_str());
        assert_eq!(trailing(&text, 0, &mut left, &mut Vec::new()), None);
        assert_eq!(left, FRAME_BYTES - ROW_BYTES);
    }

    #[test]
    fn the_band_is_on_the_primary_cursors_row_in_view() {
        let mut editor = editor("one\ntwo\nthree\n", 5);
        assert_eq!(
            editor.current_line_cells(),
            Some(CellRect {
                row: 1,
                column: 0,
                rows: 1,
                columns: 40
            })
        );
        editor.set_visual_aids(VisualAids {
            current_line: false,
            ..VisualAids::default()
        });
        assert_eq!(editor.current_line_cells(), None);
    }
}
