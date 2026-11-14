//! A review comment's draft in the top float (ADR 0122, step 3): the answer's rows grow with
//! its text, wrapped as its window wraps it, up to [`MAX_DRAFT_ROWS`], and scroll past them.
//!
//! Must not depend on: `editor`, `command` or `keymap`, or the host's window.

use super::{Kind, PromptLayout, layout};
use crate::display_map::DisplaySnapshot;
use crate::display_map::wrap_map;
use crate::frame::{CellRect, FrameLayout};

/// The most rows a draft's answer takes; past them it scrolls.
pub(crate) const MAX_DRAFT_ROWS: usize = 10;

/// Where a draft's parts go: as a prompt that reads text, its answer `rows` rows tall where
/// the frame has room below, the field and its room with it.
pub(crate) fn draft_layout(
    frame: &FrameLayout,
    label: &str,
    message: Option<&str>,
    rows: usize,
) -> PromptLayout {
    let mut placed = layout(frame, label, Kind::Draft, message);
    let tiled = frame.tiled();
    let outer = placed.regions.outer;
    // A float of one row keeps the label and the answer on it.
    if placed.regions.inner.rows <= 1 || placed.input.rows == 0 {
        return placed;
    }
    let room = (tiled.row + tiled.rows).saturating_sub(outer.row + outer.rows);
    let extra = rows.clamp(1, MAX_DRAFT_ROWS).saturating_sub(1).min(room);
    let grow = |rect: &mut CellRect| {
        if rect.rows > 0 {
            rect.rows += extra;
        }
    };
    grow(&mut placed.regions.outer);
    grow(&mut placed.regions.inner);
    grow(&mut placed.input);
    grow(&mut placed.field);
    grow(&mut placed.field_room);
    placed
}

/// The view rows the draft's text `display` takes in its window `columns` wide, broken where
/// the window's wrap breaks them, at most [`MAX_DRAFT_ROWS`]: it reads only those rows, so a
/// long draft costs no more than a short one.
pub(crate) fn draft_rows(display: &DisplaySnapshot, columns: usize) -> usize {
    wrap_map::rows_at_most(display, columns, MAX_DRAFT_ROWS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::frame::{FrameSize, single};

    fn frame() -> FrameLayout {
        single(
            FrameSize {
                rows: 40,
                columns: 100,
            },
            0,
        )
    }

    fn rows_of(text: &str, columns: usize) -> usize {
        draft_rows(&Buffer::from_text("draft", text).display(), columns)
    }

    #[test]
    fn rows_grow_with_lines_and_wrapping_and_stop_at_ten() {
        assert_eq!(rows_of("", 20), 1);
        assert_eq!(rows_of("a\nb", 20), 2);
        assert_eq!(rows_of("a\n", 20), 2, "the row after a break");
        assert_eq!(rows_of(&"x".repeat(19), 20), 1);
        assert_eq!(rows_of(&"x".repeat(20), 20), 2, "a column short");
        assert_eq!(rows_of(&"\n".repeat(50), 20), MAX_DRAFT_ROWS);
        let long = "x".repeat(10 * 1024 * 1024);
        assert_eq!(rows_of(&long, 2000), MAX_DRAFT_ROWS);
    }

    #[test]
    fn rows_break_between_words_as_the_window_does() {
        // 19 cells a row: four words of four fit; three of twelve take a row each, where
        // counting 38 cells would make two.
        assert_eq!(rows_of("aaaa bbbb cccc dddd", 20), 1);
        let words = ["a", "b", "c"].map(|letter| letter.repeat(12)).join(" ");
        assert_eq!(rows_of(&words, 20), 3);
    }

    #[test]
    fn the_answer_grows_down_and_the_label_stays() {
        let one = draft_layout(&frame(), "Reply to #7", None, 1);
        let four = draft_layout(&frame(), "Reply to #7", None, 4);
        assert_eq!(one.input.rows, 1);
        assert_eq!(four.input.rows, 4);
        assert_eq!(four.input.row, one.input.row);
        assert_eq!(four.label_cells, one.label_cells);
        assert_eq!(four.regions.outer.rows, one.regions.outer.rows + 3);
        assert_eq!(four.field.rows, 4);
        let many = draft_layout(&frame(), "Reply to #7", None, 50);
        assert_eq!(many.input.rows, MAX_DRAFT_ROWS);
    }

    #[test]
    fn a_short_frame_keeps_the_float_inside_it() {
        let frame = single(
            FrameSize {
                rows: 8,
                columns: 100,
            },
            0,
        );
        let placed = draft_layout(&frame, "Reply to #7", None, 10);
        let tiled = frame.tiled();
        let outer = placed.regions.outer;
        assert!(outer.row + outer.rows <= tiled.row + tiled.rows);
        assert!(placed.input.rows >= 1);
    }
}
