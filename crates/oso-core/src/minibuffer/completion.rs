//! Where the parts of a prompt that completes go in its float, centered in the frame
//! (ADR 0076): the label, the answer in its field, a message and the count on one row, with
//! the candidates' rows below it.
//!
//! Must not depend on: `editor`, `command` or `keymap`; it places text in cells.

use super::{MIN_INPUT_COLUMNS, PromptLayout, cells_of, cut_to};
use crate::complete::ROWS;
use crate::frame::{CellRect, FloatRegions, FrameLayout, completion_regions};

/// The cells between the label and the field.
const LABEL_GAP: usize = 1;

/// The cells a list's count takes at most, `total/total`, with a cell before it.
fn count_cells(total: usize) -> usize {
    2 * total.max(1).to_string().len() + 2
}

/// The cells the field may reach into past its own: the padding beside it, the rows of
/// padding above and below it where they are, and the cell either side of it on its row.
fn field_room(regions: &FloatRegions, field: CellRect) -> CellRect {
    let above = usize::from(regions.gap);
    let start = field.column.saturating_sub(1).max(regions.outer.column);
    let end = (field.column + field.columns + 1).min(regions.outer.column + regions.outer.columns);
    CellRect {
        row: field.row - above,
        column: start,
        rows: above + 1 + usize::from(regions.padded_below()),
        columns: end - start,
    }
}

/// Where a prompt that says `label` and completes over `total` candidates goes in a frame laid
/// out as `frame`, with `message` at the end of its answer. The label goes first on the
/// answer's row, cut with `…` to half of it at most, then the field with the answer and a
/// message, which leaves it at least 8 columns, then the count. The float has [`ROWS`]
/// candidate rows below, however many match.
pub(crate) fn completion_layout(
    frame: &FrameLayout,
    label: &str,
    message: Option<&str>,
    total: usize,
) -> PromptLayout {
    let regions = completion_regions(frame, ROWS);
    let inner = regions.inner;
    let rect = |column: usize, columns: usize| {
        let columns = if inner.rows == 0 {
            0
        } else {
            columns.min(inner.columns.saturating_sub(column))
        };
        CellRect {
            row: inner.row,
            column: inner.column + column.min(inner.columns),
            rows: usize::from(columns > 0),
            columns,
        }
    };
    let (shown, cut) = cut_to(label, inner.columns / 2);
    let label_columns = cells_of(shown) + cells_of(cut);
    let start = (label_columns + usize::from(label_columns > 0) * LABEL_GAP).min(inner.columns);
    let left = inner.columns - start;
    let count = count_cells(total).min(left.saturating_sub(MIN_INPUT_COLUMNS));
    let left = left - count;
    let message_columns = message.map_or(0, |message| {
        (cells_of(message) + 3).min(left.saturating_sub(MIN_INPUT_COLUMNS))
    });
    let field = rect(start, left);
    PromptLayout {
        regions,
        label: if label_columns > 0 {
            std::iter::once(0..shown.len()).collect()
        } else {
            Vec::new()
        },
        label_cut: !cut.is_empty(),
        label_cells: rect(0, label_columns),
        input: rect(start, left - message_columns),
        field,
        field_room: if field.columns > 0 {
            field_room(&regions, field)
        } else {
            rect(0, 0)
        },
        message: rect(start + left - message_columns, message_columns),
        keys: [rect(0, 0); 2],
        verbs: [rect(0, 0); 2],
        count: rect(inner.columns - count, count),
    }
}

#[cfg(test)]
mod tests {
    use super::completion_layout;
    use crate::frame::{CellRect, FrameSize, layout as frame_layout};

    fn end(rect: CellRect) -> usize {
        rect.column + rect.columns
    }

    fn overlaps(a: CellRect, b: CellRect) -> bool {
        a.rows > 0
            && b.rows > 0
            && a.columns > 0
            && b.columns > 0
            && a.row < b.row + b.rows
            && b.row < a.row + a.rows
            && a.column < end(b)
            && b.column < end(a)
    }

    #[test]
    fn the_label_the_field_and_the_count_share_the_answers_row() {
        let frame = frame_layout(FrameSize {
            rows: 42,
            columns: 100,
        });
        let layout = completion_layout(&frame, "M-x", Some("No match"), 340);
        let inner = layout.regions.inner;
        assert_eq!(
            layout.label_cells,
            CellRect {
                columns: 3,
                ..inner
            }
        );
        assert_eq!(layout.field.column, inner.column + 4);
        assert_eq!(layout.count.columns, "340/340".len() + 1);
        assert_eq!(end(layout.count), end(inner));
        assert_eq!(end(layout.field), layout.count.column);
        assert_eq!(end(layout.message), end(layout.field));
        assert_eq!(end(layout.input), layout.message.column);
        assert_eq!(layout.regions.candidates.rows, 18);
        // The field reaches into the padding above and below, and a cell either side.
        assert_eq!(layout.field_room.row, inner.row - 1);
        assert_eq!(layout.field_room.rows, 3);
        assert_eq!(layout.field_room.column, layout.field.column - 1);
    }

    #[test]
    fn a_long_label_is_cut_to_half_the_row() {
        let frame = frame_layout(FrameSize {
            rows: 42,
            columns: 100,
        });
        let label = "a label much longer than half of the float's row, by far and away";
        let layout = completion_layout(&frame, label, None, 3);
        assert!(layout.label_cut);
        assert_eq!(layout.label_cells.columns, layout.regions.inner.columns / 2);
    }

    proptest::proptest! {
        /// The parts stay inside the float and never overlap, from frames of no rows up.
        #[test]
        fn prop_a_completions_parts_never_overlap(
            rows in 0usize..44,
            columns in 0usize..160,
            total in 0usize..100_000,
            label in "[a-z ]{0,80}",
            message in proptest::option::of("[a-z]{0,30}"),
        ) {
            let frame = frame_layout(FrameSize { rows, columns });
            let layout = completion_layout(&frame, &label, message.as_deref(), total);
            let outer = layout.regions.outer;
            let parts = [
                layout.label_cells,
                layout.field,
                layout.count,
                layout.regions.candidates,
            ];
            for (index, part) in parts.iter().enumerate() {
                if part.rows == 0 || part.columns == 0 {
                    continue;
                }
                proptest::prop_assert!(part.row >= outer.row);
                proptest::prop_assert!(part.row + part.rows <= outer.row + outer.rows);
                proptest::prop_assert!(part.column >= outer.column && end(*part) <= end(outer));
                for other in &parts[index + 1..] {
                    proptest::prop_assert!(!overlaps(*part, *other), "{part:?} {other:?}");
                }
            }
            // The answer and a message are inside the field.
            for part in [layout.input, layout.message] {
                if part.columns > 0 {
                    proptest::prop_assert!(part.column >= layout.field.column);
                    proptest::prop_assert!(end(part) <= end(layout.field));
                }
            }
        }
    }
}
