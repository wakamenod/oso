//! Where the parts of a candidate's row go: its text, the binding after it and its annotation
//! at the end of the row, cut where the row is narrow (ADR 0076).
//!
//! Must not depend on: `editor`, `command` or `keymap`; it places text in cells.
//!
//! The annotation goes first: it is cut with `…`, and goes once fewer than
//! [`MIN_ANNOTATION`] cells of it would show. Then the binding goes, and then the text is cut with
//! `…` at the row's end. So the text is never cut while anything else shows.

use super::{CUT, cells_of, cut_to};

/// The fewest cells of an annotation a row shows, cut, before it shows none.
pub(crate) const MIN_ANNOTATION: usize = 8;

/// The cells between a candidate's text and its binding.
const BINDING_GAP: usize = 1;

/// The fewest cells between what is on the left of a row and its annotation.
const ANNOTATION_GAP: usize = 2;

/// Where the parts of one candidate's row go, in columns from the row's first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateRow {
    /// How many bytes of the candidate's text show.
    pub text: usize,
    /// Whether the text was cut, and a `…` follows what shows of it.
    pub text_cut: bool,
    /// The column of its binding, if it shows.
    pub binding: Option<usize>,
    /// The column of its annotation, how many bytes of it show and whether a `…` follows
    /// them, if it shows.
    pub annotation: Option<(usize, usize, bool)>,
}

/// Where `text`, `binding` and `annotation` go in a row of `columns` cells. `binding` and
/// `annotation` may be empty.
pub fn candidate_row(text: &str, binding: &str, annotation: &str, columns: usize) -> CandidateRow {
    let (text_cells, binding_cells) = (cells_of(text), cells_of(binding));
    let keyed = text_cells
        + if binding_cells > 0 {
            BINDING_GAP + binding_cells
        } else {
            0
        };
    let whole = |binding| CandidateRow {
        text: text.len(),
        text_cut: false,
        binding,
        annotation: None,
    };
    if !annotation.is_empty() {
        let room = columns.saturating_sub(keyed + ANNOTATION_GAP);
        let (shown, cut) = cut_to(annotation, room);
        let shown_cells = cells_of(shown) + cells_of(cut);
        if room >= MIN_ANNOTATION || (cut.is_empty() && room > 0 && shown_cells <= room) {
            let binding = (binding_cells > 0).then_some(text_cells + BINDING_GAP);
            return CandidateRow {
                annotation: Some((columns - shown_cells, shown.len(), cut == CUT)),
                ..whole(binding)
            };
        }
    }
    if binding_cells > 0 && keyed <= columns {
        return whole(Some(text_cells + BINDING_GAP));
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
    use super::{CandidateRow, MIN_ANNOTATION, candidate_row};
    use crate::minibuffer::cells_of;

    const DOC: &str = "Save the current buffer to its file";

    #[test]
    fn a_wide_row_shows_all_with_the_annotation_at_its_end() {
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 80);
        assert_eq!(row.binding, Some(12));
        assert_eq!(row.annotation, Some((80 - DOC.len(), DOC.len(), false)));
        assert_eq!((row.text, row.text_cut), ("save-buffer".len(), false));
    }

    #[test]
    fn a_narrow_row_cuts_the_annotation_then_drops_it_then_the_binding_then_cuts_the_text() {
        // 11 + 1 + 7 + 2 = 21 cells before the annotation.
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 21 + MIN_ANNOTATION);
        let (column, bytes, cut) = row.annotation.unwrap();
        assert!(cut && column + bytes + 1 == 21 + MIN_ANNOTATION);
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 21 + MIN_ANNOTATION - 1);
        assert_eq!((row.annotation, row.binding), (None, Some(12)));
        let row = candidate_row("save-buffer", "C-x C-s", DOC, 18);
        assert_eq!((row.binding, row.text, row.text_cut), (None, 11, false));
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

    proptest::proptest! {
        /// What a row shows of a candidate fits its cells, and the parts keep their order.
        #[test]
        fn prop_a_candidate_row_fits(
            text in "[a-z日-]{0,30}",
            binding in "[A-Z -]{0,10}",
            annotation in "[a-z ]{0,60}",
            columns in 0usize..100,
        ) {
            let CandidateRow { text: shown, text_cut, binding: at, annotation: note } =
                candidate_row(&text, &binding, &annotation, columns);
            let mut end = cells_of(&text[..shown]) + usize::from(text_cut);
            proptest::prop_assert!(end <= columns || columns == 0 && end == 0);
            if let Some(column) = at {
                proptest::prop_assert!(column > end - usize::from(text_cut));
                end = column + cells_of(&binding);
                proptest::prop_assert!(end <= columns);
            }
            if let Some((column, bytes, cut)) = note {
                proptest::prop_assert!(column >= end);
                proptest::prop_assert_eq!(column + cells_of(&annotation[..bytes]) + usize::from(cut), columns);
            }
        }
    }
}
