//! Where a buffer was edited since its file was read, read again or saved: the lines
//! whitespace cleanup on save looks at (ADR 0113).
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only offsets.

use std::ops::Range;

use super::Buffer;
use crate::selection::{OffsetEdit, follow_ranges};

impl Buffer {
    /// Where the buffer was edited since its file was read, read again or saved.
    pub(crate) fn edited(&self) -> &Edited {
        &self.edited
    }

    /// Forgets where it was edited, as a save does once it has its text.
    pub(crate) fn clear_edited(&mut self) {
        self.edited.clear();
    }
}

/// The ranges edited, ascending and apart. A deletion leaves an empty range where it was, so
/// its line counts too.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Edited {
    ranges: Vec<Range<usize>>,
}

impl Edited {
    /// The ranges, ascending and apart.
    pub(crate) fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    /// Forgets every range: the buffer holds what its file does.
    pub(crate) fn clear(&mut self) {
        self.ranges.clear();
    }

    /// Takes in one batch of edits, ascending and apart in the offsets before it, as
    /// [`OffsetEdit`]s are: the ranges kept follow them, and the new text of each is a
    /// range of its own. O(ranges + edits).
    pub(crate) fn follow(&mut self, edits: &[OffsetEdit]) {
        if edits.is_empty() {
            return;
        }
        follow_ranges(&mut self.ranges, edits);
        let mut shift: isize = 0;
        let new = edits.iter().map(|edit| {
            let start = edit.start.saturating_add_signed(shift);
            shift += edit.new_len.cast_signed() - edit.old_len.cast_signed();
            start..start + edit.new_len
        });
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(self.ranges.len() + edits.len());
        let mut kept = std::mem::take(&mut self.ranges).into_iter().peekable();
        let mut new = new.peekable();
        loop {
            let next = match (kept.peek(), new.peek()) {
                (Some(a), Some(b)) if a.start <= b.start => kept.next(),
                (Some(_), Some(_)) | (None, Some(_)) => new.next(),
                (Some(_), None) => kept.next(),
                (None, None) => break,
            };
            let Some(range) = next else { break };
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        self.ranges = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, old_len: usize, new_len: usize) -> OffsetEdit {
        OffsetEdit {
            start,
            old_len,
            new_len,
        }
    }

    #[test]
    fn new_text_is_a_range_and_a_deletion_an_empty_one() {
        let mut edited = Edited::default();
        edited.follow(&[edit(2, 0, 3), edit(10, 4, 0)]);
        assert_eq!(edited.ranges(), [2..5, 13..13]);
    }

    #[test]
    fn ranges_follow_later_edits_and_merge_where_they_meet() {
        let mut edited = Edited::default();
        edited.follow(&[edit(10, 0, 2)]);
        // Two bytes in front move it; one at its end joins it.
        edited.follow(&[edit(0, 0, 2), edit(12, 0, 1)]);
        assert_eq!(edited.ranges(), [0..2, 12..15]);
        // A deletion over all of it leaves an empty range at its place.
        edited.follow(&[edit(12, 3, 0)]);
        assert_eq!(edited.ranges(), [0..2, 12..12]);
        edited.clear();
        assert!(edited.ranges().is_empty());
    }

    #[test]
    fn many_cursors_typing_keep_one_range_each() {
        let mut edited = Edited::default();
        let batch: Vec<_> = (0..1000).map(|line| edit(line * 10, 0, 1)).collect();
        edited.follow(&batch);
        edited.follow(
            &batch
                .iter()
                .enumerate()
                .map(|(i, e)| edit(e.start + i + 1, 0, 1))
                .collect::<Vec<_>>(),
        );
        assert_eq!(edited.ranges().len(), 1000);
        assert_eq!(edited.ranges()[1], 12..14);
    }
}
