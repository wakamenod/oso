//! Where a buffer was edited since its file was read, read again or saved: the lines
//! whitespace cleanup on save looks at (ADR 0113). Also what a running save covered, which
//! comes back if the write fails, and the ranges a cleanup command waiting for its scan
//! covers, which follow the edits made meanwhile.
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only offsets.

use std::ops::Range;

use super::Buffer;
use crate::selection::{OffsetEdit, follow_ranges};

/// A cleanup command waiting for its scan, as the editor names it.
pub(crate) type Tracking = u64;

/// Every set of edited ranges a buffer keeps.
#[derive(Clone, Debug, Default)]
pub(crate) struct EditedLines {
    /// Since the file was read, read again, or a save took its text.
    now: Edited,
    /// What a running save took, until its write ends.
    saving: Option<Edited>,
    /// The ranges cleanup commands waiting for their scans cover.
    tracked: Vec<(Tracking, Edited)>,
}

impl EditedLines {
    /// Takes in one batch of edits: each set follows it, and its new text is edited.
    pub(crate) fn follow(&mut self, edits: &[OffsetEdit]) {
        self.now.follow(edits);
        if let Some(saving) = &mut self.saving {
            saving.follow_only(edits);
        }
        for (_, tracked) in &mut self.tracked {
            tracked.follow_only(edits);
        }
    }
}

impl Buffer {
    /// Where the buffer was edited since its file was read, read again or a save took its
    /// text, ascending and apart.
    pub(crate) fn edited_ranges(&self) -> &[Range<usize>] {
        self.edited.now.ranges()
    }

    /// Forgets every edited range: the buffer holds what its file does.
    pub(crate) fn clear_edited(&mut self) {
        self.edited.now.clear();
        self.edited.saving = None;
    }

    /// A save took the text: what was edited goes aside until the write ends, and only
    /// `kept`, the whitespace a cursor kept, stays edited.
    pub(crate) fn save_took_edits(&mut self, kept: Vec<Range<usize>>) {
        let taken = std::mem::replace(
            &mut self.edited.now,
            Edited {
                ranges: kept,
                scratch: Vec::new(),
            },
        );
        self.edited.saving = Some(taken);
    }

    /// The write a save started ended: what it took is forgotten if it `wrote`, and edited
    /// again if it did not.
    pub(crate) fn save_ended_edits(&mut self, wrote: bool) {
        if let Some(saving) = self.edited.saving.take()
            && !wrote
        {
            self.edited.now.add(saving.ranges());
        }
    }

    /// Keeps `ranges` as `id`'s, following the edits made from now on.
    pub(crate) fn track(&mut self, id: Tracking, ranges: Vec<Range<usize>>) {
        self.edited.tracked.push((
            id,
            Edited {
                ranges,
                scratch: Vec::new(),
            },
        ));
    }

    /// The ranges `id` keeps, as the edits since moved them.
    pub(crate) fn tracked(&self, id: Tracking) -> Option<&[Range<usize>]> {
        self.edited
            .tracked
            .iter()
            .find(|(tracking, _)| *tracking == id)
            .map(|(_, edited)| edited.ranges())
    }

    /// Forgets `id`'s ranges.
    pub(crate) fn untrack(&mut self, id: Tracking) {
        self.edited.tracked.retain(|(tracking, _)| *tracking != id);
    }
}

/// Ranges, ascending and apart. A deletion leaves an empty range where it was, so its line
/// counts too.
#[derive(Clone, Debug, Default)]
pub(crate) struct Edited {
    ranges: Vec<Range<usize>>,
    /// Reused for each batch, so that typing does not allocate.
    scratch: Vec<Range<usize>>,
}

impl PartialEq for Edited {
    fn eq(&self, other: &Self) -> bool {
        self.ranges == other.ranges
    }
}

impl Edited {
    /// The ranges, ascending and apart.
    pub(crate) fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    /// Forgets every range.
    pub(crate) fn clear(&mut self) {
        self.ranges.clear();
    }

    /// Moves the ranges through one batch of edits, as [`Edited::follow`] does, without
    /// taking the new text in.
    pub(crate) fn follow_only(&mut self, edits: &[OffsetEdit]) {
        follow_ranges(&mut self.ranges, edits);
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
        let mut added = edits.iter().map(|edit| {
            let start = edit.start.saturating_add_signed(shift);
            shift += edit.new_len.cast_signed() - edit.old_len.cast_signed();
            start..start + edit.new_len
        });
        self.merge_from(&mut added);
    }

    /// Takes `ranges`, ascending, in as edited too.
    pub(crate) fn add(&mut self, ranges: &[Range<usize>]) {
        self.merge_from(&mut ranges.iter().cloned());
    }

    /// Merges ascending `added` into the ranges, through the scratch list.
    fn merge_from(&mut self, added: &mut dyn Iterator<Item = Range<usize>>) {
        let mut merged = std::mem::take(&mut self.scratch);
        merged.clear();
        let mut kept = self.ranges.iter().cloned().peekable();
        let mut added = added.peekable();
        loop {
            let next = match (kept.peek(), added.peek()) {
                (Some(a), Some(b)) if a.start <= b.start => kept.next(),
                (Some(_), Some(_)) | (None, Some(_)) => added.next(),
                (Some(_), None) => kept.next(),
                (None, None) => break,
            };
            let Some(range) = next else { break };
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        self.scratch = std::mem::replace(&mut self.ranges, merged);
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
