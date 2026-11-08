//! Where the parts of a prompt that completes go in its float, centered in the frame
//! (ADR 0076): the count, the label and the answer on one row, with the candidates' rows right
//! below it.
//!
//! Must not depend on: `editor`, `command` or `keymap`; it places text in cells.

use super::{MIN_INPUT_COLUMNS, PromptLayout, cells_of, cut_to};
use crate::complete::ROWS;
use crate::frame::{CellRect, FrameLayout, completion_regions};

/// The cells between the count, the label and the answer.
const GAP: usize = 1;

/// The cells a list's count takes at most, `total/total`.
fn count_cells(total: usize) -> usize {
    2 * total.max(1).to_string().len() + 1
}

/// Where a prompt that says `label` and completes over `total` candidates goes in a frame laid
/// out as `frame`, with `message` at the end of its answer. The answer's row holds the count
/// first, kept as wide as the widest count, then the label, cut with `…` to a third of the row
/// at most, then the answer with a message at its end, which leaves it at least 8 columns.
/// There is no field around the answer. The float has [`ROWS`] candidate rows below, however
/// many match.
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
    let count = count_cells(total).min(inner.columns.saturating_sub(MIN_INPUT_COLUMNS));
    let at_label = (count + GAP).min(inner.columns);
    let (shown, cut) = cut_to(label, inner.columns / 3);
    let label_columns = cells_of(shown) + cells_of(cut);
    let start =
        (at_label + label_columns + usize::from(label_columns > 0) * GAP).min(inner.columns);
    let left = inner.columns - start;
    let message_columns = message.map_or(0, |message| {
        (cells_of(message) + 3).min(left.saturating_sub(MIN_INPUT_COLUMNS))
    });
    PromptLayout {
        regions,
        label: if label_columns > 0 {
            std::iter::once(0..shown.len()).collect()
        } else {
            Vec::new()
        },
        label_cut: !cut.is_empty(),
        label_cells: rect(at_label, label_columns),
        input: rect(start, left - message_columns),
        field: rect(0, 0),
        field_room: rect(0, 0),
        message: rect(start + left - message_columns, message_columns),
        keys: [rect(0, 0); 2],
        verbs: [rect(0, 0); 2],
        count: rect(0, count),
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
    fn the_count_the_label_and_the_answer_share_its_row_in_that_order() {
        let frame = frame_layout(FrameSize {
            rows: 42,
            columns: 100,
        });
        let layout = completion_layout(&frame, "M-x", Some("No match"), 340);
        let inner = layout.regions.inner;
        assert_eq!(
            layout.count,
            CellRect {
                columns: 7,
                ..inner
            }
        );
        assert_eq!(layout.label_cells.column, inner.column + 8);
        assert_eq!(layout.label_cells.columns, 3);
        assert_eq!(layout.input.column, inner.column + 12);
        assert_eq!(end(layout.message), end(inner));
        assert_eq!(end(layout.input), layout.message.column);
        assert_eq!(layout.field.columns, 0, "no field around the answer");
        assert_eq!(layout.regions.candidates.rows, 18);
        assert_eq!(layout.regions.candidates.row, inner.row + 1);
    }

    #[test]
    fn a_long_label_is_cut_to_a_third_of_the_row() {
        let frame = frame_layout(FrameSize {
            rows: 42,
            columns: 100,
        });
        let label = "a label much longer than half of the float's row, by far and away";
        let layout = completion_layout(&frame, label, None, 3);
        assert!(layout.label_cut);
        assert_eq!(layout.label_cells.columns, layout.regions.inner.columns / 3);
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
                layout.count,
                layout.label_cells,
                layout.input,
                layout.message,
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
        }
    }
}
