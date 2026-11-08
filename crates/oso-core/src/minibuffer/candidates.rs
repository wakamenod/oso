//! Where the parts of a candidate's row go: its text, the key after it and its annotation
//! at the end of the row, cut where the row is narrow (ADR 0076).
//!
//! Must not depend on: `editor`, `command` or `keymap`; it places text in cells.
//!
//! The annotation goes first: it is cut with `…`, and goes once fewer than
//! [`MIN_ANNOTATION`] cells of it would show. Then the key goes, and then the text is cut with
//! `…` at the row's end. So the text is never cut while anything else shows.

use super::{CUT, cells_of, cut_to};

/// The fewest cells of an annotation a row shows, cut, before it shows none.
pub const MIN_ANNOTATION: usize = 8;

/// The cells between a candidate's text and its key.
const KEY_GAP: usize = 1;

/// The fewest cells between what is on the left of a row and its annotation.
const ANNOTATION_GAP: usize = 2;

/// Where the parts of one candidate's row go, in columns from the row's first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateRow {
    /// How many bytes of the candidate's text show.
    pub text: usize,
    /// Whether the text was cut, and a `…` follows what shows of it.
    pub text_cut: bool,
    /// The column of its key, if it shows.
    pub key: Option<usize>,
    /// The column of its annotation, how many bytes of it show and whether a `…` follows
    /// them, if it shows.
    pub annotation: Option<(usize, usize, bool)>,
}

/// Where `text`, `key` and `annotation` go in a row of `columns` cells. `key` and
/// `annotation` may be empty.
pub fn candidate_row(text: &str, key: &str, annotation: &str, columns: usize) -> CandidateRow {
    let (text_cells, key_cells) = (cells_of(text), cells_of(key));
    let keyed = text_cells
        + if key_cells > 0 {
            KEY_GAP + key_cells
        } else {
            0
        };
    let whole = |key| CandidateRow {
        text: text.len(),
        text_cut: false,
        key,
        annotation: None,
    };
    if !annotation.is_empty() {
        let room = columns.saturating_sub(keyed + ANNOTATION_GAP);
        let (shown, cut) = cut_to(annotation, room);
        let shown_cells = cells_of(shown) + cells_of(cut);
        if room >= MIN_ANNOTATION || (cut.is_empty() && room > 0 && shown_cells <= room) {
            let key = (key_cells > 0).then_some(text_cells + KEY_GAP);
            return CandidateRow {
                annotation: Some((columns - shown_cells, shown.len(), cut == CUT)),
                ..whole(key)
            };
        }
    }
    if key_cells > 0 && keyed <= columns {
        return whole(Some(text_cells + KEY_GAP));
    }
    // A row of no cells has no room for a `…` either.
    if columns == 0 {
        return CandidateRow {
            text: 0,
            ..whole(None)
        };
    }
    let (shown, cut) = cut_to(text, columns);
    CandidateRow {
        text: shown.len(),
        text_cut: cut == CUT,
        key: None,
        annotation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateRow, MIN_ANNOTATION, candidate_row};
    use crate::frame::{CellRect, FrameSize, layout as frame_layout};
    use crate::minibuffer::{Kind, ListSize, cells_of, layout_with};

    const DOC: &str = "Save the current buffer to its file";

    #[test]
    fn a_wide_row_shows_all_with_the_annotation_at_its_end() {
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 80);
        assert_eq!(row.key, Some(12));
        assert_eq!(row.annotation, Some((80 - DOC.len(), DOC.len(), false)));
        assert_eq!((row.text, row.text_cut), ("save-buffer".len(), false));
    }

    #[test]
    fn a_narrow_row_cuts_the_annotation_then_drops_it_then_the_key_then_cuts_the_text() {
        // 11 + 1 + 7 + 2 = 21 cells before the annotation.
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 21 + MIN_ANNOTATION);
        let (column, bytes, cut) = row.annotation.unwrap();
        assert!(cut && column + bytes + 1 == 21 + MIN_ANNOTATION);
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 21 + MIN_ANNOTATION - 1);
        assert_eq!((row.annotation, row.key), (None, Some(12)));
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 18);
        assert_eq!((row.key, row.text, row.text_cut), (None, 11, false));
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 6);
        assert_eq!((row.text, row.text_cut), (5, true));
    }

    #[test]
    fn a_short_annotation_shows_whole_where_it_fits() {
        let row = candidate_row("undo", "", "Undo", 10);
        assert_eq!(row.annotation, Some((6, 4, false)));
        assert_eq!(candidate_row("undo", "", "Undo", 9).annotation, None);
    }

    #[test]
    fn wide_clusters_are_cut_whole() {
        let row = candidate_row("日本語のコマンド", "", "", 7);
        assert_eq!((row.text, row.text_cut), ("日本語".len(), true));
    }

    fn rect_end(rect: CellRect) -> usize {
        rect.column + rect.columns
    }

    fn overlaps(a: CellRect, b: CellRect) -> bool {
        a.rows > 0
            && b.rows > 0
            && a.columns > 0
            && b.columns > 0
            && a.row < b.row + b.rows
            && b.row < a.row + a.rows
            && a.column < rect_end(b)
            && b.column < rect_end(a)
    }

    #[test]
    fn the_count_ends_the_labels_first_row_and_the_rows_follow_the_answer() {
        let frame = frame_layout(FrameSize {
            rows: 40,
            columns: 100,
        });
        let list = ListSize {
            rows: 18,
            total: 340,
        };
        let kind = Kind::Complete { history: "command" };
        let layout = layout_with(&frame, "M-x", kind, None, Some(list));
        let inner = layout.regions.inner;
        assert_eq!(layout.count.row, inner.row);
        assert_eq!(layout.count.columns, "340/340".len() + 1);
        assert_eq!(rect_end(layout.count), rect_end(inner));
        assert!(rect_end(layout.label_cells) <= layout.count.column);
        assert_eq!(layout.regions.candidates.rows, 18);
        assert_eq!(layout.regions.candidates.row, inner.row + inner.rows + 1);
    }

    proptest::proptest! {
        /// The parts of a prompt with a list stay inside the float and never overlap, from
        /// frames of no rows up.
        #[test]
        fn prop_a_list_prompts_parts_never_overlap(
            rows in 0usize..40,
            columns in 0usize..160,
            wanted in 0usize..20,
            total in 0usize..100_000,
            label in "[a-z ]{0,80}",
            message in proptest::option::of("[a-z]{0,30}"),
        ) {
            let frame = frame_layout(FrameSize { rows, columns });
            let list = ListSize { rows: wanted, total };
            let kind = Kind::Complete { history: "command" };
            let layout = layout_with(&frame, &label, kind, message.as_deref(), Some(list));
            let outer = layout.regions.outer;
            let parts = [
                layout.label_cells,
                layout.input,
                layout.message,
                layout.count,
                layout.regions.candidates,
            ];
            for (index, part) in parts.iter().enumerate() {
                if part.rows == 0 || part.columns == 0 {
                    continue;
                }
                proptest::prop_assert!(part.row >= outer.row);
                proptest::prop_assert!(part.row + part.rows <= outer.row + outer.rows);
                proptest::prop_assert!(part.column >= outer.column && rect_end(*part) <= rect_end(outer));
                for other in &parts[index + 1..] {
                    proptest::prop_assert!(!overlaps(*part, *other), "{part:?} {other:?}");
                }
            }
        }

        /// What a row shows of a candidate fits its cells, and the parts keep their order.
        #[test]
        fn prop_a_candidate_row_fits(
            text in "[a-z日-]{0,30}",
            key in "[A-Z -]{0,10}",
            annotation in "[a-z ]{0,60}",
            columns in 0usize..100,
        ) {
            let CandidateRow { text: shown, text_cut, key: at, annotation: note } =
                candidate_row(&text, &key, &annotation, columns);
            let mut end = cells_of(&text[..shown]) + usize::from(text_cut);
            proptest::prop_assert!(end <= columns || columns == 0 && end == 0);
            if let Some(column) = at {
                proptest::prop_assert!(column > end - usize::from(text_cut));
                end = column + cells_of(&key);
                proptest::prop_assert!(end <= columns);
            }
            if let Some((column, bytes, cut)) = note {
                proptest::prop_assert!(column >= end);
                proptest::prop_assert_eq!(column + cells_of(&annotation[..bytes]) + usize::from(cut), columns);
            }
        }
    }
}
