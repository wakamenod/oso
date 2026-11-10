//! The visual aids' switches, and what they fill in the tiled window: trailing whitespace,
//! the current line's band and the cursor flash, with what decides that it flashes
//! (ADR 0097).
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use std::ops::Range;

use super::Editor;
use crate::buffer::BufferId;
use crate::display_map::DisplaySnapshot;
use crate::frame::{self, CellRect};
use crate::motion;
use crate::rope::Rope;
use crate::window::Screen;

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
    /// A band that fades after a large jump.
    pub flash: bool,
    /// Line numbers in a gutter at the tiled window's left.
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
        // Turned on again, the gutter fits the line count as it is.
        self.gutter_floor = None;
        self.fit_gutter();
    }

    /// The cells of the tiled window's trailing whitespace in view, as `screen` shows it
    /// ([`Editor::screen`]), into `out`, which is emptied first: none with whitespace off,
    /// only on a line's last view row, where spaces come before its break, and none on a line
    /// with a cursor, so that typing a space at a line's end does not flash a fill. A row whose
    /// trailing run does not end within its share of the walk gets none, so a long line costs
    /// no more than a short one.
    pub fn trailing_cells(&self, screen: &Screen, out: &mut Vec<CellRect>) {
        out.clear();
        if !self.aids.whitespace {
            return;
        }
        let (window, buffer) = self.tiled();
        let display = buffer.display();
        let text = self.frame.window.text;
        // Only the cursors in view, found as the regions' are: selections keep their order.
        let span = screen.span();
        let selections = window.selections.as_slice();
        let from = selections.partition_point(|selection| selection.cursor < span.start);
        let mut cursor_lines = selections[from..]
            .iter()
            .take_while(|selection| selection.cursor <= span.end)
            .map(|selection| display.offset_to_display_point(selection.cursor).row)
            .peekable();
        let mut left = FRAME_CHARS;
        for (index, row) in screen.rows().iter().take(text.rows).enumerate() {
            while cursor_lines.next_if(|&line| line < row.line).is_some() {}
            if row.continues || cursor_lines.peek() == Some(&row.line) {
                continue;
            }
            if let Some(range) = trailing(buffer.text(), &display, row.line, &mut left) {
                // A run that started on a row before stays unfilled there.
                let range = range.start.max(row.start)..range.end;
                frame::match_cells_on_row(text, (screen, index), &display, &range, out);
            }
        }
    }

    /// The cells of the band on the tiled window's primary cursor's line, as `screen` shows it
    /// ([`Editor::screen`]), if the current line is shown and some of it is in view: every
    /// view row the line takes, so a wrapped line has the band on each of its rows (ADR 0097).
    pub fn current_line_cells(&self, screen: &Screen) -> Option<CellRect> {
        if !self.aids.current_line {
            return None;
        }
        let (window, buffer) = self.tiled();
        let line = window.cursor_point(&buffer.display()).row;
        let text = self.frame.window.text;
        let rows = &screen.rows()[..screen.rows().len().min(text.rows)];
        let top = rows.iter().position(|row| row.line == line)?;
        let count = rows[top..]
            .iter()
            .take_while(|row| row.line == line)
            .count();
        Some(CellRect {
            row: text.row + top,
            rows: count,
            ..text
        })
    }
}

/// How far the primary cursor must move in one go, in view rows, for the cursor to flash.
const FLASH_ROWS: usize = 5;

/// What the cursor flash compares before and after an event (ADR 0097): which buffer the
/// tiled window shows and which window that is, the primary cursor's view row, the top
/// row in view, and whether isearch runs (ADR 0100).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewSample {
    buffer: BufferId,
    window: usize,
    row: usize,
    top: usize,
    searching: bool,
}

impl Editor {
    /// The tiled window's view as the cursor flash compares it.
    pub fn view_sample(&self) -> ViewSample {
        let (window, buffer) = self.tiled();
        let display = buffer.display();
        ViewSample {
            buffer: window.buffer(),
            window: self.tiled_index(),
            row: window.cursor_row(&display),
            top: window.top_row(&display),
            searching: self.isearch().is_some(),
        }
    }

    /// Whether the view went from `before` to `now` by a large jump, so the cursor flashes:
    /// another buffer or window, or the primary cursor or the view's top 5 or more rows away,
    /// as `C-v`, `M-v` and `C-l` scroll. Never while isearch runs, as its steps have their
    /// own current match, and never with the flash off. The view follows the cursor by the
    /// fewest rows, so it moves that far only when the cursor does too.
    pub fn flashes(&self, before: &ViewSample, now: &ViewSample) -> bool {
        if !self.aids.flash || now.searching {
            return false;
        }
        if (before.buffer, before.window) != (now.buffer, now.window) {
            return true;
        }
        before.row.abs_diff(now.row) >= FLASH_ROWS || before.top.abs_diff(now.top) >= FLASH_ROWS
    }

    /// The cells the flash covers: from the primary cursor to the text area's right edge, on
    /// its view row, if that row is in view as `screen` shows it ([`Editor::screen`]).
    pub fn flash_cells(&self, screen: &Screen) -> Option<CellRect> {
        let (window, buffer) = self.tiled();
        let text = self.frame.window.text;
        let point = window.cursor_point(&buffer.display());
        let row = screen.row_of(point).filter(|&row| row < text.rows)?;
        let column = point
            .column
            .checked_sub(screen.origin(&screen.rows()[row]))
            .filter(|&c| c < text.columns)?;
        Some(CellRect {
            row: text.row + row,
            column: text.column + column,
            rows: 1,
            columns: text.columns - column,
        })
    }
}

/// How many bytes a step of the walk back reads.
const STEP: usize = 64;

/// The trailing whitespace of logical line `row`: spaces, tabs and U+3000 before its line's
/// break or the end of the text. `None` if there is none, or if the run does not end within its share of what is
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
        editor.trailing_cells(&editor.screen_now(), &mut cells);
        // The cursor's line keeps its two spaces unmarked; the tab, the full-width space
        // before a `\r\n`, and a line of nothing but spaces are filled.
        assert_eq!(rows(&cells), [(1, 1, 7), (2, 1, 2), (4, 0, 2)]);
        editor.set_visual_aids(VisualAids {
            whitespace: false,
            ..VisualAids::default()
        });
        editor.trailing_cells(&editor.screen_now(), &mut cells);
        assert!(cells.is_empty());
    }

    #[test]
    fn a_trailing_run_longer_than_the_walk_is_left_unfilled_and_long_lines_cost_little() {
        let long = format!("x{}\nshort  \n", " ".repeat(ROW_CHARS + 10));
        let editor = editor(&long, long.len());
        let mut cells = Vec::new();
        editor.trailing_cells(&editor.screen_now(), &mut cells);
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
    fn a_jump_of_five_rows_flashes_and_a_step_does_not() {
        let text = "line\n".repeat(40);
        let mut editor = editor(&text, 0);
        let before = editor.view_sample();
        editor.current_mut().0.selections = SelectionSet::new(5 * 4);
        assert!(!editor.flashes(&before, &editor.view_sample()), "four rows");
        editor.current_mut().0.selections = SelectionSet::new(5 * 5);
        assert!(editor.flashes(&before, &editor.view_sample()), "five rows");
        editor.set_visual_aids(VisualAids {
            flash: false,
            ..VisualAids::default()
        });
        assert!(!editor.flashes(&before, &editor.view_sample()), "off");
    }

    #[test]
    fn a_scroll_of_five_rows_flashes_and_following_a_step_does_not() {
        let text = "line\n".repeat(80);
        let editor = editor(&text, 5 * 20);
        let mut before = editor.view_sample();
        let mut now = before;
        now.top += 8;
        assert!(editor.flashes(&before, &now), "C-l: the cursor stayed");
        now.row += 2;
        assert!(
            editor.flashes(&before, &now),
            "M-v: the cursor kept to the view"
        );
        now = before;
        (now.row, now.top) = (now.row + 1, now.top + 1);
        assert!(!editor.flashes(&before, &now), "following a step");
        before.buffer = BufferId(99);
        assert!(
            editor.flashes(&before, &editor.view_sample()),
            "another buffer"
        );
    }

    #[test]
    fn isearch_never_flashes() {
        let editor = editor(&"line\n".repeat(40), 0);
        let before = editor.view_sample();
        let mut now = before;
        now.row = 30;
        now.searching = true;
        assert!(!editor.flashes(&before, &now));
    }

    #[test]
    fn the_flash_runs_from_the_cursor_to_the_right_edge() {
        let editor = editor("abcdef\nxy\n", 9);
        assert_eq!(
            editor.flash_cells(&editor.screen_now()),
            Some(CellRect {
                row: 1,
                column: 2,
                rows: 1,
                columns: 38
            })
        );
    }

    #[test]
    fn the_band_is_on_the_primary_cursors_row_in_view() {
        let mut editor = editor("one\ntwo\nthree\n", 5);
        assert_eq!(
            editor.current_line_cells(&editor.screen_now()),
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
        assert_eq!(editor.current_line_cells(&editor.screen_now()), None);
    }
}
