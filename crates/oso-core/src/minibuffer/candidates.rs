//! Where the parts of a candidate's row go: its text, its binding after it in parentheses, and
//! its annotation in a column the rows shown share, cut where the row is narrow (ADR 0076).
//!
//! Must not depend on: `editor`, `command` or `keymap`; it places text in cells.
//!
//! The annotations start at one column, after the longest text and binding of the rows shown,
//! and at most halfway across, so they read as a column. A row whose own text reaches past it
//! starts its annotation further on. The annotation goes first: it is cut with `…` at the row's
//! end, and goes once fewer than [`MIN_ANNOTATION`] cells of it would show. Then the binding
//! goes, and then the text is cut with `…` at the row's end. So the text is never cut while
//! anything else shows.

use super::{CUT, cells_of, cut_to};

/// The fewest cells of an annotation a row shows, cut, before it shows none.
pub(crate) const MIN_ANNOTATION: usize = 8;

/// The cells between a candidate's text and its binding.
const BINDING_GAP: usize = 1;

/// The fewest cells between what is on the left of a row and its annotation.
const ANNOTATION_GAP: usize = 2;

/// What opens a binding shown after a candidate: `(C-x C-s)`.
pub const BINDING_OPEN: &str = "(";

/// What closes it.
pub const BINDING_CLOSE: &str = ")";

/// Where the parts of one candidate's row go, in columns from the row's first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateRow {
    /// How many bytes of the candidate's text show.
    pub text: usize,
    /// Whether the text was cut, and a `…` follows what shows of it.
    pub text_cut: bool,
    /// The column of its binding's opening parenthesis, if it shows.
    pub binding: Option<usize>,
    /// The column of its annotation, how many bytes of it show and whether a `…` follows
    /// them, if it shows.
    pub annotation: Option<(usize, usize, bool)>,
}

/// The cells of `text` and of `binding` after it, in parentheses.
fn keyed_cells(text: &str, binding: &str) -> usize {
    let binding = cells_of(binding);
    cells_of(text)
        + if binding > 0 {
            BINDING_GAP + binding + 2
        } else {
            0
        }
}

/// The column the annotations of rows of `columns` cells start at: after the longest text and
/// binding among `rows`, and at most halfway across.
pub fn annotation_column<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
    columns: usize,
) -> usize {
    let longest = rows
        .into_iter()
        .map(|(text, binding)| keyed_cells(text, binding))
        .max()
        .unwrap_or(0);
    (longest + ANNOTATION_GAP).min(columns / 2)
}

/// Where `text`, `binding` and `annotation` go in a row of `columns` cells, with the annotation
/// at `column` or, past the text and binding, further on. `binding` and `annotation` may be
/// empty.
pub fn candidate_row(
    text: &str,
    binding: &str,
    annotation: &str,
    columns: usize,
    column: usize,
) -> CandidateRow {
    let (text_cells, binding_cells) = (cells_of(text), cells_of(binding));
    let keyed = keyed_cells(text, binding);
    let whole = |binding| CandidateRow {
        text: text.len(),
        text_cut: false,
        binding,
        annotation: None,
    };
    let binding_at = (binding_cells > 0).then_some(text_cells + BINDING_GAP);
    if !annotation.is_empty() {
        let at = column.max(keyed + ANNOTATION_GAP);
        let room = columns.saturating_sub(at);
        let (shown, cut) = cut_to(annotation, room);
        if room >= MIN_ANNOTATION || (cut.is_empty() && room > 0) {
            return CandidateRow {
                annotation: Some((at, shown.len(), cut == CUT)),
                ..whole(binding_at)
            };
        }
    }
    if binding_cells > 0 && keyed <= columns {
        return whole(binding_at);
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
        binding: None,
        annotation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateRow, MIN_ANNOTATION, annotation_column, candidate_row};
    use crate::minibuffer::cells_of;

    const DOC: &str = "Save the current buffer to its file";

    #[test]
    fn annotations_start_at_one_column_after_the_longest_text_and_binding() {
        let rows = [
            ("undo", "C-z"),
            ("save-buffer", "C-x C-s"),
            ("recover-session", ""),
        ];
        // `save-buffer (C-x C-s)` is 21 cells, and 2 more before the annotation.
        assert_eq!(annotation_column(rows, 100), 23);
        assert_eq!(annotation_column(rows, 30), 15, "at most halfway");
        assert_eq!(annotation_column([], 100), 2);
    }

    #[test]
    fn a_wide_row_shows_all_with_the_annotation_at_the_column() {
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 80, 30);
        assert_eq!(row.binding, Some(12));
        assert_eq!(row.annotation, Some((30, DOC.len(), false)));
        assert_eq!((row.text, row.text_cut), ("save-buffer".len(), false));
        // A row whose text reaches past the column starts its annotation further on.
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 80, 10);
        assert_eq!(row.annotation.map(|(at, ..)| at), Some(23));
    }

    #[test]
    fn a_narrow_row_cuts_the_annotation_then_drops_it_then_the_binding_then_cuts_the_text() {
        // 11 + 1 + 9 + 2 = 23 cells before the annotation.
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 23 + MIN_ANNOTATION, 0);
        let (column, bytes, cut) = row.annotation.unwrap();
        assert!(cut && column == 23 && column + bytes + 1 == 23 + MIN_ANNOTATION);
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 23 + MIN_ANNOTATION - 1, 0);
        assert_eq!((row.annotation, row.binding), (None, Some(12)));
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 20, 0);
        assert_eq!((row.binding, row.text, row.text_cut), (None, 11, false));
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 6, 0);
        assert_eq!((row.text, row.text_cut), (5, true));
    }

    #[test]
    fn a_short_annotation_shows_whole_where_it_fits() {
        let row = candidate_row("undo", "", "Undo", 10, 0);
        assert_eq!(row.annotation, Some((6, 4, false)));
        assert_eq!(candidate_row("undo", "", "Undo", 9, 0).annotation, None);
    }

    #[test]
    fn wide_clusters_are_cut_whole() {
        let row = candidate_row("日本語のコマンド", "", "", 7, 0);
        assert_eq!((row.text, row.text_cut), ("日本語".len(), true));
    }

    proptest::proptest! {
        /// What a row shows of a candidate fits its cells, and the parts keep their order.
        #[test]
        fn prop_a_candidate_row_fits(
            text in "[a-z日-]{0,30}",
            binding in "[A-Z -]{0,10}",
            annotation in "[a-z ]{0,60}",
            columns in 0usize..100,
            column in 0usize..60,
        ) {
            let CandidateRow { text: shown, text_cut, binding: at, annotation: note } =
                candidate_row(&text, &binding, &annotation, columns, column);
            let mut end = cells_of(&text[..shown]) + usize::from(text_cut);
            proptest::prop_assert!(end <= columns || columns == 0 && end == 0);
            if let Some(at) = at {
                proptest::prop_assert!(at > end - usize::from(text_cut));
                end = at + cells_of(&binding) + 2;
                proptest::prop_assert!(end <= columns);
            }
            if let Some((at, bytes, cut)) = note {
                proptest::prop_assert!(at >= end && at >= column);
                let shown = cells_of(&annotation[..bytes]) + usize::from(cut);
                proptest::prop_assert!(at + shown <= columns);
            }
        }
    }
}
