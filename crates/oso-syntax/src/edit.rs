//! A batch of a buffer's edits as tree-sitter's `InputEdit`s, with the points tree-sitter
//! keeps beside byte offsets (ADR 0111).
//!
//! Must not depend on: `document`; it sees only the texts before and after a batch.

use oso_core::rope::{Point, Rope};
use oso_core::syntax::Splice;
use tree_sitter::InputEdit;

/// The edits that take a tree of `before` to one of `after`, in the order to apply them, or
/// `None` if `splices` do not take `before` to a text of `after`'s length, so that the tree
/// must be parsed again from nothing.
///
/// The splices are ascending, apart and in `before`'s offsets. Applied highest first, each one's
/// start and old end are still where `before` has them, as the ones applied so far lie after
/// it. Its new end is its start moved by the extent of its new text, read in `after`, where
/// the splices below it have moved it. O(log n) a splice.
pub(crate) fn input_edits(
    before: &Rope,
    after: &Rope,
    splices: &[Splice],
) -> Option<Vec<InputEdit>> {
    // Where each splice's new text starts in `after`: the splices below it moved it.
    let mut new_starts = Vec::with_capacity(splices.len());
    let (mut old_end, mut shift) = (0usize, 0isize);
    for splice in splices {
        let end = splice.start.checked_add(splice.old_len)?;
        if splice.start < old_end || end > before.len() {
            return None;
        }
        new_starts.push(splice.start.checked_add_signed(shift)?);
        shift = shift
            .checked_add(isize::try_from(splice.new_len).ok()?)?
            .checked_sub(isize::try_from(splice.old_len).ok()?)?;
        old_end = end;
    }
    if before.len().checked_add_signed(shift)? != after.len() {
        return None;
    }
    let edits = splices
        .iter()
        .zip(new_starts)
        .rev()
        .filter(|(splice, _)| splice.old_len > 0 || splice.new_len > 0)
        .map(|(splice, new_start)| {
            let start_position = before.offset_to_point(splice.start);
            let extent = extent(after, new_start, new_start + splice.new_len);
            InputEdit {
                start_byte: splice.start,
                old_end_byte: splice.start + splice.old_len,
                new_end_byte: splice.start + splice.new_len,
                start_position: point(start_position),
                old_end_position: point(before.offset_to_point(splice.start + splice.old_len)),
                new_end_position: point(if extent.row == 0 {
                    Point::new(start_position.row, start_position.column + extent.column)
                } else {
                    Point::new(start_position.row + extent.row, extent.column)
                }),
            }
        })
        .collect();
    Some(edits)
}

/// The lines and the bytes after the last line break that `start..end` of `text` spans.
fn extent(text: &Rope, start: usize, end: usize) -> Point {
    let (from, to) = (text.offset_to_point(start), text.offset_to_point(end));
    if from.row == to.row {
        Point::new(0, to.column.saturating_sub(from.column))
    } else {
        Point::new(to.row - from.row, to.column)
    }
}

pub(crate) fn point(point: Point) -> tree_sitter::Point {
    tree_sitter::Point::new(point.row, point.column)
}

#[cfg(test)]
mod tests {
    use oso_core::rope::Rope;
    use oso_core::syntax::Splice;

    use super::input_edits;

    fn splice(start: usize, old_len: usize, new_len: usize) -> Splice {
        Splice {
            start,
            old_len,
            new_len,
        }
    }

    #[test]
    fn edits_come_highest_first_with_their_points() {
        let before = Rope::from("ab\ncd\nef");
        // "b" becomes "X\nY", and "e" goes.
        let after = Rope::from("aX\nY\ncd\nf");
        let edits = input_edits(&before, &after, &[splice(1, 1, 3), splice(6, 1, 0)]).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(
            (
                edits[0].start_byte,
                edits[0].old_end_byte,
                edits[0].new_end_byte
            ),
            (6, 7, 6)
        );
        assert_eq!(edits[0].start_position, tree_sitter::Point::new(2, 0));
        assert_eq!(edits[0].new_end_position, tree_sitter::Point::new(2, 0));
        assert_eq!(
            (
                edits[1].start_byte,
                edits[1].old_end_byte,
                edits[1].new_end_byte
            ),
            (1, 2, 4)
        );
        assert_eq!(edits[1].old_end_position, tree_sitter::Point::new(0, 2));
        assert_eq!(edits[1].new_end_position, tree_sitter::Point::new(1, 1));
    }

    #[test]
    fn splices_that_do_not_fit_give_none() {
        let before = Rope::from("abcd");
        assert!(
            input_edits(
                &before,
                &Rope::from("abcd"),
                &[splice(2, 1, 1), splice(1, 1, 1)]
            )
            .is_none()
        );
        assert!(input_edits(&before, &Rope::from("abcd"), &[splice(4, 1, 1)]).is_none());
        assert!(input_edits(&before, &Rope::from("abcde"), &[splice(0, 1, 1)]).is_none());
        assert!(input_edits(&before, &Rope::from("abcd"), &[]).is_some_and(|e| e.is_empty()));
    }
}
