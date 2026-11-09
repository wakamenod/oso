//! Offsets moved through a batch of edits, for cursors, marks and the ranges an edit
//! changed (ADR 0053, ADR 0091).
//!
//! Must not depend on: `buffer`, `motion` or `window`; it sees only offsets.

use std::ops::Range;

use crate::sum_tree::Bias;

/// One replacement as a cursor sees it: `old_len` bytes at `start` became
/// `new_len` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OffsetEdit {
    pub(crate) start: usize,
    pub(crate) old_len: usize,
    pub(crate) new_len: usize,
}

/// Moves `ranges`, ascending and apart, through `edits` as markers: a start stays
/// before text inserted at it, and an end goes after it (ADR 0091).
pub(crate) fn follow_ranges(ranges: &mut [Range<usize>], edits: &[OffsetEdit]) {
    let mut starts = Follow::new(edits, Bias::Left);
    let mut ends = Follow::new(edits, Bias::Right);
    for range in ranges {
        range.start = starts.map(range.start);
        range.end = ends.map(range.end);
    }
}

/// Where new text ends after `edits`, ascending. The edits are as in
/// [`SelectionSet::follow_edits`](super::SelectionSet::follow_edits), and deletions have no new text.
pub(crate) fn new_text_ends(edits: &[OffsetEdit]) -> impl Iterator<Item = usize> + '_ {
    let (mut old_end, mut new_end) = (0, 0);
    edits.iter().filter_map(move |edit| {
        new_end += edit.start - old_end + edit.new_len;
        old_end = edit.start + edit.old_len;
        (edit.new_len > 0).then_some(new_end)
    })
}

/// Moves offsets through sorted edits, as [`SelectionSet::follow_edits`](super::SelectionSet::follow_edits) says. The
/// offsets must come in ascending order, and cost O(offsets + edits) together.
pub(super) struct Follow<'a> {
    edits: &'a [OffsetEdit],
    bias: Bias,
    /// Where the edits passed so far end, before and after them.
    old_end: usize,
    new_end: usize,
    next: usize,
}

impl<'a> Follow<'a> {
    pub(super) fn new(edits: &'a [OffsetEdit], bias: Bias) -> Self {
        Self {
            edits,
            bias,
            old_end: 0,
            new_end: 0,
            next: 0,
        }
    }

    pub(super) fn map(&mut self, offset: usize) -> usize {
        loop {
            let Some(edit) = self.edits.get(self.next) else {
                return self.new_end.saturating_add(offset - self.old_end);
            };
            let new_start = self.new_end + (edit.start - self.old_end);
            let before = match self.bias {
                Bias::Left => offset <= edit.start,
                Bias::Right => offset < edit.start,
            };
            if before {
                return self.new_end.saturating_add(offset - self.old_end);
            }
            if offset < edit.start + edit.old_len {
                return match self.bias {
                    Bias::Left => new_start,
                    Bias::Right => new_start + edit.new_len,
                };
            }
            self.old_end = edit.start + edit.old_len;
            self.new_end = new_start + edit.new_len;
            self.next += 1;
        }
    }
}
