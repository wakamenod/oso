//! The tiled window's gutter (ADR 0097): how wide line numbers make it, laying the frame out
//! again when that changes, and the numbers of the rows in view, built each frame as the
//! modeline's text is.
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use std::fmt::Write as _;
use std::ops::Range;

use super::Editor;
use crate::frame;

/// The fewest digits the line-number column takes.
const MIN_DIGITS: usize = 3;

/// The gutter's rows in view, from the window's top: each a line's number, right-aligned
/// with a cell of space after it, or blank. Reused from frame to frame.
#[derive(Debug, Default)]
pub struct Gutter {
    text: String,
    rows: Vec<(Range<usize>, bool)>,
}

impl Gutter {
    /// Each row's text, blank past the buffer's end and on the rows a line continues on, and
    /// whether it is the primary cursor's line.
    pub fn rows(&self) -> impl Iterator<Item = (&str, bool)> {
        self.rows
            .iter()
            .map(|(range, current)| (&self.text[range.clone()], *current))
    }
}

/// The columns line numbers take for a buffer of `lines` lines: its digits, at least 3, and a
/// cell of space.
fn columns_for(lines: usize) -> usize {
    let mut digits = 1;
    let mut rest = lines / 10;
    while rest > 0 {
        digits += 1;
        rest /= 10;
    }
    digits.max(MIN_DIGITS) + 1
}

impl Editor {
    /// The gutter's columns for the tiled window as it is: none with line numbers off.
    pub(super) fn gutter_columns(&self) -> usize {
        if !self.aids.line_numbers {
            return 0;
        }
        columns_for(self.tiled().1.text().line_count())
    }

    /// Lays the frame out again if the gutter's width changed, as when the line count gains
    /// a digit by an edit, a load or another buffer shown, or line numbers were turned on.
    /// Returns whether it did, so a frame is drawn.
    pub fn fit_gutter(&mut self) -> bool {
        // As the layout gives it: none in a frame too narrow for it and a column of text.
        let size = self.frame.size();
        let wanted = frame::fitting_gutter(size, self.gutter_columns());
        if wanted == self.frame.window.gutter.columns {
            return false;
        }
        let before = self.frame;
        self.set_frame_size(size);
        self.frame != before
    }

    /// Fills in the gutter's rows in view, reusing `gutter`'s room: none while it has no
    /// columns. A frame draws logical lines until soft wrap's next step (M2-25 step 4), which
    /// leaves blank the rows a line continues on, so each row drawn is a line's first.
    pub fn gutter_into(&self, gutter: &mut Gutter) {
        gutter.text.clear();
        gutter.rows.clear();
        let columns = self.frame.window.gutter.columns;
        if columns == 0 {
            return;
        }
        let (window, buffer) = self.tiled();
        let display = buffer.display();
        let current = window.cursor_point(&display).row;
        let top = window.drawn_top(&display).row;
        let end = display.row_count().min(top + self.frame.window.text.rows);
        for row in top..end {
            let start = gutter.text.len();
            let _ = write!(gutter.text, "{:>1$} ", row + 1, columns - 1);
            gutter.rows.push((start..gutter.text.len(), row == current));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::VisualAids;
    use crate::frame::FrameSize;
    use crate::selection::SelectionSet;
    use crate::window::ViewSize;

    fn numbered(text: &str, rows: usize) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
        editor.set_view_size(ViewSize::new(rows, 20));
        editor.set_visual_aids(VisualAids {
            line_numbers: true,
            ..VisualAids::default()
        });
        editor
    }

    fn rows(editor: &Editor) -> Vec<(String, bool)> {
        let mut gutter = Gutter::default();
        editor.gutter_into(&mut gutter);
        gutter
            .rows()
            .map(|(text, current)| (text.to_owned(), current))
            .collect()
    }

    #[test]
    fn a_gutter_is_the_digits_at_least_three_and_a_space() {
        assert_eq!(columns_for(1), 4);
        assert_eq!(columns_for(999), 4);
        assert_eq!(columns_for(1000), 5);
        assert_eq!(columns_for(123_456), 7);
    }

    #[test]
    fn numbers_are_right_aligned_and_the_current_line_is_marked() {
        let mut editor = numbered("a\nb\nc", 5);
        editor.current_mut().0.selections = SelectionSet::new(2);
        assert_eq!(
            rows(&editor),
            [
                ("  1 ".to_owned(), false),
                ("  2 ".to_owned(), true),
                ("  3 ".to_owned(), false),
            ],
            "no rows past the buffer's end"
        );
    }

    #[test]
    fn the_text_area_gives_the_gutter_its_columns_and_floats_keep_their_place() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "a\n"));
        editor.set_frame_size(FrameSize {
            rows: 10,
            columns: 80,
        });
        let area = editor.frame().window.area();
        editor.set_visual_aids(VisualAids {
            line_numbers: true,
            ..VisualAids::default()
        });
        let frame = editor.frame();
        assert_eq!(
            (frame.window.gutter.columns, frame.window.text.column),
            (4, 4)
        );
        assert_eq!(frame.window.text.columns, 76);
        assert_eq!(frame.window.area(), area);
        assert_eq!(editor.window().size().columns, 76);
    }

    #[test]
    fn a_line_count_that_gains_a_digit_widens_the_gutter() {
        let mut editor = numbered(&"x\n".repeat(998), 5);
        assert!(!editor.fit_gutter(), "999 lines");
        let id = editor.window().buffer();
        let edit = crate::buffer::Replacement {
            range: 0..0,
            text: "\n",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        assert!(!editor.fit_gutter(), "the edit laid it out at 1000 lines");
        assert_eq!(editor.frame().window.gutter.columns, 5);
        assert_eq!(rows(&editor)[0].0, "   1 ");
    }

    #[test]
    fn another_buffer_shown_takes_its_own_gutter_at_once() {
        let mut editor = numbered(&"x\n".repeat(1000), 5);
        assert_eq!(editor.frame().window.gutter.columns, 5);
        let small = editor.add_buffer(Buffer::from_text("small", "a"));
        editor.show(small);
        assert_eq!(editor.frame().window.gutter.columns, 4);
        assert_eq!(editor.window().size().columns, 16);
    }

    #[test]
    fn a_frame_narrower_than_the_gutter_is_laid_out_once() {
        let mut editor = numbered("a", 5);
        editor.set_frame_size(FrameSize {
            rows: 5,
            columns: 3,
        });
        assert_eq!(editor.frame().window.gutter.columns, 0, "no cut numbers");
        assert!(!editor.fit_gutter());
    }

    #[test]
    fn line_numbers_off_take_no_columns() {
        let editor = Editor::with_buffer(Buffer::from_text("t", "a"));
        assert_eq!(editor.frame().window.gutter.columns, 0);
        assert!(rows(&editor).is_empty());
    }
}
