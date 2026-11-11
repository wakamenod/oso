//! Each tiled window's gutter (ADR 0097, ADR 0112): how wide line numbers make it, laying the frame out
//! again when that changes, and the numbers of the rows in view, built each frame as the
//! modeline's text is.
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use std::fmt::Write as _;
use std::ops::Range;

use super::Editor;
use crate::frame;
use crate::window::{Screen, Window, WindowId};

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
    /// The gutter's columns for `window` as it is: none with line numbers off, and otherwise
    /// as its own buffer's line count needs (ADR 0112). They never narrow while it shows one
    /// buffer, so that a line count crossing a digit boundary back and forth, as a newline
    /// typed and deleted at 1000 lines, does not change the text area's width at each key,
    /// which would wrap its rows again (ADR 0104, ADR 0105).
    pub(super) fn gutter_columns(&self, window: &Window) -> usize {
        if !self.aids.line_numbers {
            return 0;
        }
        let own = columns_for(self.shown(window).text().line_count());
        match window.gutter_floor() {
            Some((id, floor)) if id == window.buffer() => own.max(floor),
            _ => own,
        }
    }

    /// Lays the frame out again if the gutter's width changed, as when the line count gains
    /// a digit by an edit, a load or another buffer shown, or line numbers were turned on.
    /// Returns whether it did, so a frame is drawn.
    pub fn fit_gutter(&mut self) -> bool {
        // As the layout gives it: none in a window too narrow for it and a column of text.
        let size = self.frame.size();
        let changed = self.windows[..self.tiled_count()].iter().any(|window| {
            let Some(regions) = self.frame.regions(window.id()) else {
                return false;
            };
            let room = frame::FrameSize {
                rows: size.rows,
                columns: regions.area().columns,
            };
            frame::fitting_gutter(room, self.gutter_columns(window)) != regions.gutter.columns
        });
        if !changed {
            return false;
        }
        let before = self.frame.clone();
        // Within the command's budget: a narrower text area wraps the rows in view again,
        // and the rest waits for the job (ADR 0103).
        self.lay_out(size);
        self.frame != before
    }

    /// Fills in the gutter's rows in view of the tiled window `id`, as `screen` shows them
    /// ([`Editor::screen_of`]), reusing `gutter`'s room: none while it has no columns. A line's
    /// number is on its first view row, and the rows it continues on stay blank (ADR 0100).
    pub fn gutter_into(&self, id: WindowId, screen: &Screen, gutter: &mut Gutter) {
        gutter.text.clear();
        gutter.rows.clear();
        let Some((window, buffer, regions)) = self.window_in_tree(id) else {
            return;
        };
        let columns = regions.gutter.columns;
        if columns == 0 {
            return;
        }
        let current = window.cursor_point(&buffer.display()).row;
        for row in screen.rows().iter().take(regions.text.rows) {
            let start = gutter.text.len();
            if row.starts_line() {
                let _ = write!(gutter.text, "{:>1$} ", row.line + 1, columns - 1);
            }
            gutter
                .rows
                .push((start..gutter.text.len(), row.line == current));
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
        editor.gutter_into(editor.tiled().0.id(), &editor.screen_now(), &mut gutter);
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
        let area = editor.tiled_regions().area();
        editor.set_visual_aids(VisualAids {
            line_numbers: true,
            ..VisualAids::default()
        });
        let window = editor.tiled_regions();
        assert_eq!((window.gutter.columns, window.text.column), (4, 4));
        assert_eq!(window.text.columns, 76);
        assert_eq!(window.area(), area);
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
        assert_eq!(editor.tiled_regions().gutter.columns, 5);
        assert_eq!(rows(&editor)[0].0, "   1 ");
    }

    #[test]
    fn a_gutter_that_gains_a_digit_in_a_command_takes_from_its_budget() {
        // One row, so that the rows near the view cost less than a budget.
        let mut editor = numbered(&"x\n".repeat(998), 1);
        let id = editor.window().buffer();
        editor.set_wraps(id, true);
        let columns = editor.window().size().columns;
        editor.begin_undo_group();
        // What the command's edits left: none.
        editor
            .current_mut()
            .0
            .wrap_map_mut()
            .unwrap()
            .spend_budget();
        let edit = crate::buffer::Replacement {
            range: 0..0,
            text: "\n",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        assert_eq!(
            editor.window().size().columns,
            columns - 1,
            "1000 lines, 4 digits"
        );
        let left = editor.window().wrap_map().unwrap().budget_left();
        assert_eq!(
            left, 0,
            "the narrower rows wait for the job, as a command's edits do"
        );
        editor.end_undo_group();
    }

    #[test]
    fn a_gutter_does_not_narrow_while_its_buffer_is_edited() {
        let mut editor = numbered(&"x\n".repeat(999), 5);
        assert_eq!(editor.tiled_regions().gutter.columns, 5, "1000 lines");
        let id = editor.window().buffer();
        let delete = crate::buffer::Replacement {
            range: 0..2,
            text: "",
        };
        editor.apply_edits(id, &[delete]).unwrap();
        editor.fit_gutter();
        assert_eq!(editor.tiled_regions().gutter.columns, 5, "999 lines, still");
        let other = editor.add_buffer(Buffer::from_text("other", "a"));
        editor.show(other);
        editor.show(id);
        assert_eq!(
            editor.tiled_regions().gutter.columns,
            4,
            "shown again, it fits"
        );
    }

    #[test]
    fn another_buffer_shown_takes_its_own_gutter_at_once() {
        let mut editor = numbered(&"x\n".repeat(1000), 5);
        assert_eq!(editor.tiled_regions().gutter.columns, 5);
        let small = editor.add_buffer(Buffer::from_text("small", "a"));
        editor.show(small);
        assert_eq!(editor.tiled_regions().gutter.columns, 4);
        assert_eq!(editor.window().size().columns, 16);
    }

    #[test]
    fn a_frame_narrower_than_the_gutter_is_laid_out_once() {
        let mut editor = numbered("a", 5);
        editor.set_frame_size(FrameSize {
            rows: 5,
            columns: 3,
        });
        assert_eq!(editor.tiled_regions().gutter.columns, 0, "no cut numbers");
        assert!(!editor.fit_gutter());
    }

    #[test]
    fn line_numbers_off_take_no_columns() {
        let editor = Editor::with_buffer(Buffer::from_text("t", "a"));
        assert_eq!(editor.tiled_regions().gutter.columns, 0);
        assert!(rows(&editor).is_empty());
    }
}
