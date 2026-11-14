//! A selection of a terminal's cells with the mouse (ADR 0120, step 4b-2), from the cell
//! pressed to the cell the pointer is over, in reading order. Cells are anchored to their line
//! from the top of the scrollback, so a selection stays on its text while output scrolls it.
//! A hash of each selected line's text on the screen is kept, so that output into the
//! selection clears it; lines off the screen are forgotten, so it costs the rows shown.
//!
//! Must not depend on: `app`, the renderer or the terminal's thread; the grid marks and reads
//! the cells.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::{Range, RangeInclusive};

/// A cell by its line from the top of the scrollback, and its column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Spot {
    pub line: usize,
    pub column: u16,
}

/// The cells between where the drag began and where the pointer is.
#[derive(Debug)]
pub(crate) struct Selection {
    anchor: Spot,
    head: Spot,
    /// A hash of each selected line's row text on the screen when it was first seen, by line.
    texts: BTreeMap<usize, u64>,
}

impl Selection {
    /// A selection that begins at `at` and holds nothing until the pointer moves.
    pub(crate) fn new(at: Spot) -> Selection {
        Selection {
            anchor: at,
            head: at,
            texts: BTreeMap::new(),
        }
    }

    /// Moves the end that follows the pointer to `to`. Returns whether it moved.
    pub(crate) fn extend(&mut self, to: Spot) -> bool {
        let moved = self.head != to;
        self.head = to;
        moved
    }

    /// Whether it holds no cell: the pointer has not left the cell pressed.
    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The lines it holds.
    pub(crate) fn lines(&self) -> RangeInclusive<usize> {
        let (start, end) = self.ends();
        start.line..=end.line
    }

    /// The columns it holds of `line`, in a row `columns` wide; `None` for none.
    pub(crate) fn columns(&self, line: usize, columns: u16) -> Option<Range<u16>> {
        let (start, end) = self.ends();
        if self.is_empty() || line < start.line || line > end.line {
            return None;
        }
        let from = if line == start.line { start.column } else { 0 };
        let to = if line == end.line {
            end.column.saturating_add(1)
        } else {
            columns
        };
        let range = from.min(columns)..to.min(columns);
        (!range.is_empty()).then_some(range)
    }

    /// Whether `line`'s text is known already.
    pub(crate) fn knows(&self, line: usize) -> bool {
        self.texts.contains_key(&line)
    }

    /// Keeps `text` as `line`'s, the first time the line is seen selected.
    pub(crate) fn note(&mut self, line: usize, text: &str) {
        self.texts.entry(line).or_insert_with(|| hash(text));
    }

    /// Forgets the lines outside both the selection and `shown`.
    pub(crate) fn keep(&mut self, shown: &Range<usize>) {
        let lines = self.lines();
        self.texts
            .retain(|line, _| lines.contains(line) && shown.contains(line));
    }

    /// Whether `line` is selected and its text is no longer `text`: output went into it.
    pub(crate) fn changed(&self, line: usize, text: &str) -> bool {
        self.lines().contains(&line)
            && self
                .texts
                .get(&line)
                .is_some_and(|kept| *kept != hash(text))
    }

    /// How many lines' texts are kept, for the tests of its cost.
    #[cfg(test)]
    pub(crate) fn known(&self) -> usize {
        self.texts.len()
    }

    /// The first and last cells, in reading order.
    fn ends(&self) -> (Spot, Spot) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// A line's text as the selection keeps it: the same text always gives the same hash.
fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::{Selection, Spot};

    fn spot(line: usize, column: u16) -> Spot {
        Spot { line, column }
    }

    #[test]
    fn a_selection_holds_the_cells_between_its_ends_in_reading_order() {
        let mut selection = Selection::new(spot(10, 5));
        assert!(selection.is_empty());
        assert_eq!(selection.columns(10, 80), None, "nothing until it moves");
        selection.extend(spot(12, 2));
        assert_eq!(selection.columns(10, 80), Some(5..80));
        assert_eq!(selection.columns(11, 80), Some(0..80));
        assert_eq!(selection.columns(12, 80), Some(0..3));
        assert_eq!(selection.columns(13, 80), None);
        // Backwards, the cell pressed stays selected.
        selection.extend(spot(10, 1));
        assert_eq!(selection.columns(10, 80), Some(1..6));
        assert_eq!(selection.columns(11, 80), None);
        // Past the row's end, as after a resize.
        selection.extend(spot(10, 200));
        assert_eq!(selection.columns(10, 80), Some(5..80));
    }

    #[test]
    fn output_into_a_selected_line_is_seen() {
        let mut selection = Selection::new(spot(3, 0));
        selection.extend(spot(4, 3));
        selection.note(3, "one");
        selection.note(4, "two");
        selection.note(4, "later");
        assert!(!selection.changed(3, "one"));
        assert!(!selection.changed(4, "two"), "the first text is kept");
        assert!(selection.changed(4, "tw0"));
        assert!(!selection.changed(9, "anything"), "not selected");
        selection.extend(spot(3, 2));
        selection.keep(&(0..100));
        assert!(!selection.knows(4), "no longer selected");
        selection.extend(spot(9, 0));
        selection.note(9, "nine");
        selection.keep(&(5..10));
        assert!(!selection.knows(3) && selection.knows(9), "off the screen");
    }
}
