//! A review comment's draft in the top float (ADR 0122, step 3): the answer's rows grow with
//! its text, wrapped, up to [`MAX_DRAFT_ROWS`], and scroll past them.
//!
//! Must not depend on: `editor`, `command` or `keymap`, or the host's window.

use super::{Kind, PromptLayout, cut::cells_of, layout};
use crate::frame::{CellRect, FrameLayout};
use crate::rope::Rope;

/// The most rows a draft's answer takes; past them it scrolls.
pub(crate) const MAX_DRAFT_ROWS: usize = 10;

/// The most bytes of a draft looked at to count its rows: more than [`MAX_DRAFT_ROWS`] rows
/// of the widest float hold, so that a long draft costs no more than a short one.
const LOOKED_AT: usize = 64 * 1024;

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

/// The rows `text` takes wrapped `columns` wide, as a window's wrap breaks a row a column short
/// of its width, at most [`MAX_DRAFT_ROWS`]. Counts cells, not words, which is close enough for
/// a panel's height, and looks at no more than [`LOOKED_AT`] bytes.
pub(crate) fn draft_rows(text: &Rope, columns: usize) -> usize {
    let width = columns.saturating_sub(1).max(1);
    let mut rows = 1;
    let mut cells = 0;
    let mut looked = 0;
    for chunk in text.chunks() {
        for c in chunk.chars() {
            looked += c.len_utf8();
            if looked > LOOKED_AT || rows >= MAX_DRAFT_ROWS {
                return MAX_DRAFT_ROWS;
            }
            if c == '\n' {
                rows += 1;
                cells = 0;
                continue;
            }
            let mut buf = [0; 4];
            let wide = cells_of(c.encode_utf8(&mut buf));
            if cells + wide > width {
                rows += 1;
                cells = 0;
            }
            cells += wide;
        }
    }
    rows.min(MAX_DRAFT_ROWS)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn rows_grow_with_lines_and_wrapping_and_stop_at_ten() {
        assert_eq!(draft_rows(&Rope::from(""), 20), 1);
        assert_eq!(draft_rows(&Rope::from("a\nb"), 20), 2);
        assert_eq!(
            draft_rows(&Rope::from("a\n"), 20),
            2,
            "the row after a break"
        );
        assert_eq!(draft_rows(&Rope::from(&*"x".repeat(19)), 20), 1);
        assert_eq!(
            draft_rows(&Rope::from(&*"x".repeat(20)), 20),
            2,
            "a column short"
        );
        assert_eq!(
            draft_rows(&Rope::from(&*"\n".repeat(50)), 20),
            MAX_DRAFT_ROWS
        );
        let long = "x".repeat(10 * 1024 * 1024);
        assert_eq!(draft_rows(&Rope::from(&*long), 2000), MAX_DRAFT_ROWS);
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
