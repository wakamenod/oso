//! The visual aids' switches, and what they fill in the tiled window: trailing whitespace and
//! the current line's band (ADR 0097).
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use std::ops::Range;

use super::Editor;
use crate::display_map::DisplaySnapshot;
use crate::frame::{self, CellRect};
use crate::motion;
use crate::rope::Rope;

/// The most whitespace one row's walk back over its trailing run may pass.
const ROW_CHARS: usize = 512;

/// The most whitespace a frame's walks may pass in all.
const FRAME_CHARS: usize = 16_384;

/// Which visual aids are on (ADR 0097). Every window reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualAids {
    /// Tabs, full-width and other non-ASCII spaces marked, and trailing whitespace filled.
    pub whitespace: bool,
    /// A band on the primary cursor's row.
    pub current_line: bool,
    /// A band that fades after a large jump. Not drawn yet: a later step of M2-24 draws it.
    pub flash: bool,
    /// Line numbers in a gutter. Not drawn yet: a later step of M2-24 draws them.
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
        // Logical lines, as the frame draws them until it draws view rows (ADR 0103).
        let (text, scroll) = (self.frame.window.text, window.drawn_top(&display));
        let end = (scroll.row + text.rows).min(display.row_count());
        // Only the cursors in view, found as the regions' are: selections keep their order.
        let first = motion::line_start(&display, scroll.row);
        let selections = window.selections.as_slice();
        let from = selections.partition_point(|selection| selection.cursor < first);
        let mut cursor_rows = selections[from..]
            .iter()
            .map(|selection| display.offset_to_display_point(selection.cursor).row)
            .take_while(|&row| row < end)
            .peekable();
        let mut left = FRAME_CHARS;
        for row in scroll.row..end {
            while cursor_rows.next_if(|&cursor| cursor < row).is_some() {}
            if cursor_rows.peek() == Some(&row) {
                continue;
            }
            if let Some(range) = trailing(buffer.text(), &display, row, &mut left) {
                frame::match_cells(text, scroll, &display, std::slice::from_ref(&range), out);
            }
        }
    }

    /// The cells of the band on the tiled window's primary cursor's line, if the current
    /// line is shown and some of it is in view: every row the line takes, so a wrapped line
    /// has the band on each of its rows (ADR 0097).
    pub fn current_line_cells(&self) -> Option<CellRect> {
        if !self.aids.current_line {
            return None;
        }
        let (window, buffer) = self.tiled();
        let display = buffer.display();
        let (text, scroll) = (self.frame.window.text, window.drawn_top(&display));
        let rope = buffer.text();
        let line = rope.offset_to_point(window.selections.primary().cursor).row;
        let first = display.offset_to_display_point(rope.line_start(line)).row;
        let last = display.offset_to_display_point(rope.line_end(line)).row;
        let view = scroll.row..scroll.row + text.rows;
        let (top, bottom) = (first.max(view.start), (last + 1).min(view.end));
        (top < bottom).then(|| CellRect {
            row: text.row + (top - scroll.row),
            rows: bottom - top,
            ..text
        })
    }
}

/// How many bytes a step of the walk back reads.
const STEP: usize = 64;

/// The trailing whitespace of display row `row`: spaces, tabs and U+3000 before its line's
/// break or the end of the text. `None` if there is none, if the row does not end its line,
/// as a wrapped row will not (M2-25), or if the run does not end within its share of what is
/// `left` of the frame's walk, which this takes from. The walk reads back from the row's end
/// a step at a time and stops at the first character that is not whitespace, so a short run
/// costs little whatever the line's length.
fn trailing(
    text: &Rope,
    display: &DisplaySnapshot,
    row: usize,
    left: &mut usize,
) -> Option<Range<usize>> {
    let (start, end) = (
        motion::line_start(display, row),
        motion::line_end(display, row),
    );
    let ends_line = end == text.len()
        || text
            .chunks_in_range(end..end + 1)
            .next()
            .is_some_and(|after| after.starts_with(['\n', '\r']));
    if !ends_line {
        return None;
    }
    let budget = ROW_CHARS.min(*left);
    let (mut run, mut at, mut walked) = (end, end, 0);
    // Continuation bytes read of a character not yet whole: U+3000 is E3 80 80.
    let mut pending = 0;
    'walk: while at > start {
        // A step that starts inside a character is widened to its start.
        let mut bytes = [0u8; STEP + 3];
        let mut read = 0;
        for chunk in text.chunks_in_range(at.saturating_sub(STEP).max(start)..at) {
            bytes[read..read + chunk.len()].copy_from_slice(chunk.as_bytes());
            read += chunk.len();
        }
        let from = at - read;
        for (index, &byte) in bytes[..read].iter().enumerate().rev() {
            match (byte, pending) {
                (b' ' | b'\t', 0) => run = from + index,
                (0x80, 0 | 1) => {
                    pending += 1;
                    continue;
                }
                (0xe3, 2) => {
                    pending = 0;
                    run = from + index;
                }
                _ => break 'walk,
            }
            walked += 1;
            if walked > budget {
                *left -= budget;
                return None;
            }
        }
        at = from;
    }
    *left -= walked;
    (run < end).then_some(run..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::display_map::{DisplayMap, TabWidth};
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
    fn a_trailing_run_longer_than_the_walk_is_left_unfilled_and_long_lines_cost_little() {
        let long = format!("x{}\nshort  \n", " ".repeat(ROW_CHARS + 10));
        let editor = editor(&long, long.len());
        let mut cells = Vec::new();
        editor.trailing_cells(&mut cells);
        assert_eq!(
            rows(&cells),
            [(1, 5, 2)],
            "the long run is not reached in time"
        );
        // A long line of text with two trailing spaces costs the walk two characters.
        let text = format!("{}  \n", "é".repeat(5000));
        let rope = Rope::from(text.as_str());
        let display = DisplayMap::new(&rope, TabWidth::default()).snapshot();
        let mut left = FRAME_CHARS;
        assert_eq!(
            trailing(&rope, &display, 0, &mut left),
            Some(10_000..10_002)
        );
        assert_eq!(left, FRAME_CHARS - 2);
    }

    #[test]
    fn a_full_width_space_split_across_steps_is_still_whitespace() {
        // The steps read back 64 bytes at a time; put U+3000 across a step's edge.
        let text = format!("x\u{3000}{}\n", " ".repeat(STEP - 1));
        let rope = Rope::from(text.as_str());
        let display = DisplayMap::new(&rope, TabWidth::default()).snapshot();
        let mut left = FRAME_CHARS;
        assert_eq!(
            trailing(&rope, &display, 0, &mut left),
            Some(1..text.len() - 1)
        );
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
