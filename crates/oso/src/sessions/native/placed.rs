//! Where a native session's rows are in its buffer (ADR 0165): each row's end, in bytes, and
//! the bytes a change replaces, so that the host draws a row where it is and nothing else.
//!
//! Must not depend on: the editor or the UI; the host gives how to find a line's start.
//!
//! - Rows are only added at the end, so an added row goes after the last.
//! - A reply that grew is drawn again from the start of its line `lines` on, found by the
//!   host in the buffer's text; what the row held before that line stays.

use std::ops::Range;

use super::rows::RowChange;

/// Each drawn row's end in the buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    ends: Vec<usize>,
}

impl Placed {
    /// How many bytes the rows take.
    pub fn end(&self) -> usize {
        self.ends.last().copied().unwrap_or(0)
    }

    /// Where row `row` starts.
    pub fn start(&self, row: usize) -> usize {
        row.checked_sub(1)
            .and_then(|before| self.ends.get(before).copied())
            .unwrap_or(0)
    }

    /// The row that holds byte `offset`, `None` past the last.
    pub fn row_at(&self, offset: usize) -> Option<usize> {
        let row = self.ends.partition_point(|end| *end <= offset);
        (row < self.ends.len()).then_some(row)
    }

    /// The bytes `change` replaces: none for a row added at the end, the whole row for one
    /// that changed, and from its line `lines` for one that grew, `line(start, lines)` being
    /// the start of the line `lines` lines after `start` in the buffer. `None` for a change
    /// to a row not drawn, or one added past the end.
    pub fn range(
        &self,
        change: RowChange,
        line: impl FnOnce(usize, usize) -> usize,
    ) -> Option<Range<usize>> {
        let at = change.row();
        match change {
            RowChange::Added(_) => (at == self.ends.len()).then(|| self.end()..self.end()),
            RowChange::Changed(_) => (at < self.ends.len()).then(|| self.start(at)..self.ends[at]),
            RowChange::Grew { lines, .. } => (at < self.ends.len()).then(|| {
                let (start, end) = (self.start(at), self.ends[at]);
                line(start, lines).clamp(start, end)..end
            }),
        }
    }

    /// Notes that `range` of row `at` was replaced by `length` bytes: the rows after it move.
    pub fn put(&mut self, at: usize, range: Range<usize>, length: usize) {
        let end = range.start + length;
        if at == self.ends.len() {
            self.ends.push(end);
            return;
        }
        let Some(old) = self.ends.get(at).copied() else {
            return;
        };
        for later in &mut self.ends[at..] {
            *later = (*later + end).saturating_sub(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::rows::RowChange;
    use super::Placed;

    #[test]
    fn a_byte_is_in_the_row_whose_end_is_past_it() {
        let mut placed = Placed::default();
        placed.put(0, 0..0, 4);
        placed.put(1, 4..4, 6);
        assert_eq!(placed.range(RowChange::Added(2), |_, _| 0), Some(10..10));
        assert_eq!(
            [0, 3, 4, 9, 10].map(|at| placed.row_at(at)),
            [Some(0), Some(0), Some(1), Some(1), None]
        );
        assert_eq!((placed.start(0), placed.start(1)), (0, 4));
    }
}
