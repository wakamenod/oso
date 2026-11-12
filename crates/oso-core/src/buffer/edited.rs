//! Where a buffer was edited since its file was read, read again or saved: the lines
//! whitespace cleanup on save looks at (ADR 0113). Also what a running save covered, which
//! comes back if the write fails, and the ranges a cleanup command waiting for its scan
//! covers, which follow the edits made meanwhile. An undo staged over several turns follows them
//! on a copy (ADR 0119).
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only offsets.

use std::fmt;
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
    /// How many times a save took the ranges, they were cleared, covered or tracked, ever:
    /// what a copy followed meanwhile cannot take in.
    changes: u64,
    /// How many saves that failed gave back what they took, ever.
    returned: u64,
}

/// A copy of the edited lines that follows an undo staged over several turns (ADR 0119).
#[derive(Debug)]
pub(crate) struct FollowedLines {
    lines: EditedLines,
    changes: u64,
    returned: u64,
}

impl FollowedLines {
    /// Takes in one more batch of edits, as [`EditedLines::follow`] does.
    pub(crate) fn follow(&mut self, edits: &[OffsetEdit]) {
        self.lines.follow(edits);
    }

    /// How many ranges a batch more moves, at most: a batch near the gap moves only those
    /// near it.
    pub(crate) fn len(&self) -> usize {
        let lines = &self.lines;
        lines.now.len()
            + lines.saving.as_ref().map_or(0, Edited::len)
            + lines
                .tracked
                .iter()
                .map(|(_, tracked)| tracked.len())
                .sum::<usize>()
    }

    /// Whether `now`, the buffer's, changed since the copy began in a way it cannot take in.
    pub(crate) fn stale(&self, now: &EditedLines) -> bool {
        now.changes != self.changes
    }

    /// The copy, with what happened to `now` meanwhile: a save that ended, and cleanup
    /// commands whose scans ended.
    pub(crate) fn into_lines(self, now: &EditedLines) -> EditedLines {
        let mut lines = self.lines;
        if now.saving.is_none()
            && let Some(saving) = lines.saving.take()
            && now.returned != self.returned
        {
            lines.now.add(saving.ranges());
        }
        lines
            .tracked
            .retain(|(id, _)| now.tracked.iter().any(|(other, _)| other == id));
        lines.returned = now.returned;
        lines
    }
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

    /// A copy to follow an undo staged over several turns.
    pub(crate) fn followed(&self) -> FollowedLines {
        FollowedLines {
            lines: self.clone(),
            changes: self.changes,
            returned: self.returned,
        }
    }
}

impl Buffer {
    /// Where the buffer was edited since its file was read, read again or a save took its
    /// text, ascending and apart.
    pub(crate) fn edited_ranges(&self) -> Vec<Range<usize>> {
        self.edited.now.to_vec()
    }

    /// Forgets every edited range: the buffer holds what its file does.
    pub(crate) fn clear_edited(&mut self) {
        self.edited.now.clear();
        self.edited.saving = None;
        self.edited.changes += 1;
    }

    /// A save took the text: what was edited goes aside until the write ends, and only
    /// `kept`, the whitespace a cursor kept, stays edited.
    pub(crate) fn save_took_edits(&mut self, mut kept: Vec<Range<usize>>) {
        join_touching(&mut kept);
        let taken = std::mem::replace(&mut self.edited.now, Edited::from_ranges(kept));
        self.edited.saving = Some(taken);
        self.edited.changes += 1;
    }

    /// The write a save started ended: what it took is forgotten if it `wrote`, and edited
    /// again if it did not.
    pub(crate) fn save_ended_edits(&mut self, wrote: bool) {
        if let Some(saving) = self.edited.saving.take()
            && !wrote
        {
            self.edited.now.add(saving.ranges());
            self.edited.returned += 1;
        }
    }

    /// Counts `span` as edited whole: the ranges within or touching it become one. A command
    /// that makes many edits close together, as `!` does, keeps the set from growing by one
    /// range for each (ADR 0113).
    pub(crate) fn cover_edited(&mut self, span: Range<usize>) {
        self.edited.now.cover(span);
        self.edited.changes += 1;
    }

    /// Keeps `ranges` as `id`'s, following the edits made from now on.
    pub(crate) fn track(&mut self, id: Tracking, ranges: Vec<Range<usize>>) {
        self.edited.tracked.push((id, Edited::from_ranges(ranges)));
        self.edited.changes += 1;
    }

    /// The ranges `id` keeps, as the edits since moved them.
    pub(crate) fn tracked(&self, id: Tracking) -> Option<Vec<Range<usize>>> {
        self.edited
            .tracked
            .iter()
            .find(|(tracking, _)| *tracking == id)
            .map(|(_, edited)| edited.to_vec())
    }

    /// Forgets `id`'s ranges.
    pub(crate) fn untrack(&mut self, id: Tracking) {
        self.edited.tracked.retain(|(tracking, _)| *tracking != id);
    }
}

/// Ranges, ascending and apart. A deletion leaves an empty range where it was, so its line
/// counts too.
///
/// They are kept in two parts, around a gap where the last edit was: the ranges before it as
/// they are, and the ranges after it, nearest last, less the shift the edits since have made.
/// An edit moves only the ranges between the gap and itself, and those it touches, so edits
/// one after another through the text, as a macro's replay makes, cost O(log ranges) each,
/// amortized, however many ranges there are. A batch costs O(ranges + edits) at most, as one that spans
/// the text moves the gap over every range.
#[derive(Default)]
pub(crate) struct Edited {
    /// The ranges before the gap, ascending.
    before: Vec<Range<usize>>,
    /// The ranges after the gap, descending, each less [`Edited::shift`].
    after: Vec<Range<usize>>,
    /// How far the edits since moved the ranges after the gap, wrapping: a range there is
    /// at its offsets plus this.
    shift: usize,
    /// Reused for each batch, so that typing does not allocate.
    scratch: Vec<Range<usize>>,
}

impl Clone for Edited {
    /// The ranges only: the scratch list is of no use to a copy.
    fn clone(&self) -> Self {
        Self {
            before: self.before.clone(),
            after: self.after.clone(),
            shift: self.shift,
            scratch: Vec::new(),
        }
    }
}

impl PartialEq for Edited {
    fn eq(&self, other: &Self) -> bool {
        self.ranges().eq(other.ranges())
    }
}

impl fmt::Debug for Edited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.ranges()).finish()
    }
}

impl Edited {
    /// `ranges`, ascending and apart, as they are, with the gap after the last.
    fn from_ranges(ranges: Vec<Range<usize>>) -> Self {
        Self {
            before: ranges,
            ..Self::default()
        }
    }

    /// The ranges, ascending and apart. O(ranges) to read through.
    pub(crate) fn ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        let shift = self.shift;
        self.before.iter().cloned().chain(
            self.after
                .iter()
                .rev()
                .map(move |range| moved(range, shift)),
        )
    }

    /// The ranges, ascending and apart, as a list. O(ranges).
    pub(crate) fn to_vec(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::with_capacity(self.len());
        ranges.extend_from_slice(&self.before);
        ranges.extend(
            self.after
                .iter()
                .rev()
                .map(|range| moved(range, self.shift)),
        );
        ranges
    }

    /// How many ranges there are.
    pub(crate) fn len(&self) -> usize {
        self.before.len() + self.after.len()
    }

    /// Forgets every range.
    pub(crate) fn clear(&mut self) {
        self.before.clear();
        self.after.clear();
        self.shift = 0;
    }

    /// Moves the gap so that the ranges before it are those that end before `offset`.
    /// O(log ranges + the ranges it passes).
    fn seek(&mut self, offset: usize) {
        let kept = self.before.partition_point(|range| range.end < offset);
        let back = self.shift.wrapping_neg();
        self.after.extend(
            self.before
                .drain(kept..)
                .rev()
                .map(|range| moved(&range, back)),
        );
        let shift = self.shift;
        let from = self
            .after
            .partition_point(|range| moved(range, shift).end >= offset);
        self.before.extend(
            self.after
                .drain(from..)
                .rev()
                .map(|range| moved(&range, shift)),
        );
    }

    /// Takes the ranges after the gap that start by `end` off it, ascending, onto `out`.
    fn take_after(&mut self, end: usize, out: &mut Vec<Range<usize>>) {
        let shift = self.shift;
        let from = self
            .after
            .partition_point(|range| moved(range, shift).start > end);
        out.extend(
            self.after
                .drain(from..)
                .rev()
                .map(|range| moved(&range, shift)),
        );
    }

    /// Takes the ranges that end at `start` or after and start by `end` off both parts,
    /// ascending, onto `out`, each moved once; the gap is then where they were. O(log ranges
    /// + the ranges taken and those the gap passes).
    fn take_span(&mut self, start: usize, end: usize, out: &mut Vec<Range<usize>>) {
        // Before the gap, those in the span go straight to `out`, and those past it behind
        // the gap.
        let kept = self.before.partition_point(|range| range.end < start);
        let upto = kept + self.before[kept..].partition_point(|range| range.start <= end);
        let back = self.shift.wrapping_neg();
        self.after.extend(
            self.before
                .drain(upto..)
                .rev()
                .map(|range| moved(&range, back)),
        );
        if upto == kept {
            self.seek(start);
        } else {
            out.extend(self.before.drain(kept..));
        }
        self.take_after(end, out);
    }

    /// Takes the ranges `edits` may move other than by their whole shift into the scratch
    /// list, ascending, and moves them through the edits; those after the edits shift with
    /// the gap. The ranges that end before the first edit stay before the gap, as no edit
    /// moves them.
    fn take_touched(&mut self, edits: &[OffsetEdit]) {
        let (Some(first), Some(last)) = (edits.first(), edits.last()) else {
            return;
        };
        let mut touched = std::mem::take(&mut self.scratch);
        touched.clear();
        self.take_span(first.start, last.start + last.old_len, &mut touched);
        follow_ranges(&mut touched, edits);
        self.scratch = touched;
        // Past the last edit, an offset moves by what the edits added and took away.
        for edit in edits {
            self.shift = self
                .shift
                .wrapping_add(edit.new_len)
                .wrapping_sub(edit.old_len);
        }
    }

    /// Moves the ranges through one batch of edits, as [`Edited::follow`] does, without
    /// taking the new text in.
    pub(crate) fn follow_only(&mut self, edits: &[OffsetEdit]) {
        if edits.is_empty() {
            return;
        }
        self.take_touched(edits);
        self.before.append(&mut self.scratch);
    }

    /// Takes in one batch of edits, ascending and apart in the offsets before it, as
    /// [`OffsetEdit`]s are: the ranges kept follow them, and the new text of each is a
    /// range of its own. O(log ranges + the ranges between the gap and the edits, those the
    /// edits touch, and the edits).
    pub(crate) fn follow(&mut self, edits: &[OffsetEdit]) {
        if edits.is_empty() {
            return;
        }
        self.take_touched(edits);
        let mut shift: isize = 0;
        let added = edits.iter().map(|edit| {
            let start = edit.start.saturating_add_signed(shift);
            shift += edit.new_len.cast_signed() - edit.old_len.cast_signed();
            start..start + edit.new_len
        });
        // The ranges the edits touched and their new text lie past every range before the
        // gap and before every one after it, so they merge only with each other.
        let mut touched = std::mem::take(&mut self.scratch);
        merge_onto(&mut self.before, touched.drain(..), added);
        self.scratch = touched;
    }

    /// Makes the ranges within or touching `span` one range, with `span`. O(the ranges
    /// between the gap and `span`, and those in it).
    pub(crate) fn cover(&mut self, span: Range<usize>) {
        let mut within = std::mem::take(&mut self.scratch);
        within.clear();
        self.take_span(span.start, span.end, &mut within);
        let (start, end) = within
            .iter()
            .fold((span.start, span.end), |(start, end), range| {
                (start.min(range.start), end.max(range.end))
            });
        self.before.push(start..end);
        self.scratch = within;
    }

    /// Takes `ranges`, ascending, in as edited too. O(ranges kept and added).
    pub(crate) fn add(&mut self, ranges: impl Iterator<Item = Range<usize>>) {
        let mut merged = std::mem::take(&mut self.scratch);
        merged.clear();
        merge_onto(&mut merged, self.ranges(), ranges);
        self.clear();
        self.scratch = std::mem::replace(&mut self.before, merged);
    }
}

/// Joins each range of ascending `ranges` that touches the one before with it, in place, as
/// a batch's merge would join them.
pub(crate) fn join_touching(ranges: &mut Vec<Range<usize>>) {
    let mut kept: usize = 0;
    for index in 0..ranges.len() {
        let range = ranges[index].clone();
        match kept.checked_sub(1).map(|last| &mut ranges[last]) {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => {
                ranges[kept] = range;
                kept += 1;
            }
        }
    }
    ranges.truncate(kept);
}

/// `range` moved `by`, wrapping.
fn moved(range: &Range<usize>, by: usize) -> Range<usize> {
    range.start.wrapping_add(by)..range.end.wrapping_add(by)
}

/// Pushes ascending `a` and `b` onto `out` in order, each range that touches the one before
/// joined with it.
fn merge_onto(
    out: &mut Vec<Range<usize>>,
    a: impl Iterator<Item = Range<usize>>,
    b: impl Iterator<Item = Range<usize>>,
) {
    let (mut a, mut b) = (a.peekable(), b.peekable());
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if x.start <= y.start => a.next(),
            (Some(_) | None, Some(_)) => b.next(),
            (Some(_), None) => a.next(),
            (None, None) => break,
        };
        let Some(range) = next else { break };
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(edited: &Edited) -> Vec<Range<usize>> {
        edited.ranges().collect()
    }

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
        assert_eq!(all(&edited), [2..5, 13..13]);
    }

    #[test]
    fn ranges_follow_later_edits_and_merge_where_they_meet() {
        let mut edited = Edited::default();
        edited.follow(&[edit(10, 0, 2)]);
        // Two bytes in front move it; one at its end joins it.
        edited.follow(&[edit(0, 0, 2), edit(12, 0, 1)]);
        assert_eq!(all(&edited), [0..2, 12..15]);
        // A deletion over all of it leaves an empty range at its place.
        edited.follow(&[edit(12, 3, 0)]);
        assert_eq!(all(&edited), [0..2, 12..12]);
        edited.clear();
        assert_eq!(edited.len(), 0);
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
        assert_eq!(edited.len(), 1000);
        assert_eq!(edited.ranges().nth(1), Some(12..14));
    }

    #[test]
    fn a_batch_leaves_nothing_behind_for_the_next() {
        // A batch's scratch list is the next one's: an empty batch takes nothing from it.
        let mut edited = Edited::default();
        edited.follow(&[edit(0, 0, 4)]);
        edited.follow(&[edit(2, 0, 0)]);
        edited.follow_only(&[]);
        assert_eq!(all(&edited), [Range { start: 0, end: 4 }]);
        let mut taken = std::mem::take(&mut edited);
        taken.follow_only(&[]);
        taken.follow_only(&[edit(10, 0, 1)]);
        assert_eq!(all(&taken), [Range { start: 0, end: 4 }]);
    }

    #[test]
    fn touching_ranges_join_in_place() {
        let mut ranges = vec![0..2, 2..3, 5..5, 5..7, 9..9];
        join_touching(&mut ranges);
        assert_eq!(ranges, [0..3, 5..7, 9..9]);
    }

    #[test]
    fn a_span_covers_the_ranges_in_it_as_one() {
        let mut edited = Edited::default();
        edited.follow(&[
            edit(0, 0, 1),
            edit(10, 0, 1),
            edit(20, 0, 1),
            edit(40, 0, 1),
        ]);
        edited.cover(5..22);
        assert_eq!(all(&edited), [0..1, 5..23, 43..44]);
        edited.cover(30..35);
        assert_eq!(all(&edited), [0..1, 5..23, 30..35, 43..44]);
    }

    /// The ranges kept as one list, which each batch moves whole, as before the gap.
    #[derive(Default)]
    struct Plain(Vec<Range<usize>>);

    impl Plain {
        fn merge(&mut self, added: impl Iterator<Item = Range<usize>>) {
            let kept = std::mem::take(&mut self.0);
            merge_onto(&mut self.0, kept.into_iter(), added);
        }

        fn follow(&mut self, edits: &[OffsetEdit]) {
            if edits.is_empty() {
                return;
            }
            follow_ranges(&mut self.0, edits);
            let mut shift: isize = 0;
            let added: Vec<_> = edits
                .iter()
                .map(|edit| {
                    let start = edit.start.saturating_add_signed(shift);
                    shift += edit.new_len.cast_signed() - edit.old_len.cast_signed();
                    start..start + edit.new_len
                })
                .collect();
            self.merge(added.into_iter());
        }

        fn cover(&mut self, span: Range<usize>) {
            let first = self.0.partition_point(|range| range.end < span.start);
            let last = self.0.partition_point(|range| range.start <= span.end);
            let (start, end) = self.0[first..last]
                .iter()
                .fold((span.start, span.end), |(start, end), range| {
                    (start.min(range.start), end.max(range.end))
                });
            self.0.splice(first..last, std::iter::once(start..end));
        }
    }

    /// Where an edit or a span starts, before a step of -1, 0 or 1: at a range's start or end,
    /// where the last edit started, at either end of the text, or anywhere.
    #[derive(Clone, Copy, Debug)]
    enum At {
        RangeStart(usize),
        RangeEnd(usize),
        LastEdit,
        Start,
        End,
        Anywhere(usize),
    }

    /// How much an edit deletes: a few bytes, or the rest of the text.
    #[derive(Clone, Copy, Debug)]
    enum Old {
        Short(usize),
        Rest,
    }

    #[derive(Clone, Copy, Debug)]
    struct RawEdit {
        at: At,
        step: isize,
        old: Old,
        new: usize,
    }

    #[derive(Clone, Debug)]
    enum Op {
        Follow(Vec<RawEdit>),
        Cover(At, usize),
        Add(Vec<usize>),
        /// A save takes the ranges, which then only follow the edits, and the ranges given
        /// stay edited, as the whitespace a cursor kept does.
        Take(Vec<usize>),
        /// The save failed and gives them back.
        GiveBack,
        /// A cleanup command waits with these ranges, sorted, which may touch or repeat.
        Track(Vec<usize>),
        Clone,
        Clear,
    }

    fn at() -> impl proptest::strategy::Strategy<Value = At> {
        use proptest::prelude::*;
        prop_oneof![
            3 => any::<usize>().prop_map(At::RangeStart),
            3 => any::<usize>().prop_map(At::RangeEnd),
            2 => Just(At::LastEdit),
            1 => Just(At::Start),
            1 => Just(At::End),
            2 => any::<usize>().prop_map(At::Anywhere),
        ]
    }

    fn op() -> impl proptest::strategy::Strategy<Value = Op> {
        use proptest::prelude::*;
        let old = prop_oneof![5 => (0..6usize).prop_map(Old::Short), 1 => Just(Old::Rest)];
        let new = prop_oneof![8 => 0..6usize, 1 => (1usize << 20)..(1 << 20) + 8];
        let edit = (at(), -1..=1isize, old, new).prop_map(|(at, step, old, new)| RawEdit {
            at,
            step,
            old,
            new,
        });
        let points = || proptest::collection::vec(any::<usize>(), 0..8);
        prop_oneof![
            8 => proptest::collection::vec(edit, 1..4).prop_map(Op::Follow),
            2 => (at(), 0..30usize).prop_map(|(at, len)| Op::Cover(at, len)),
            1 => points().prop_map(Op::Add),
            1 => points().prop_map(Op::Take),
            1 => Just(Op::GiveBack),
            1 => points().prop_map(Op::Track),
            1 => Just(Op::Clone),
            1 => Just(Op::Clear),
        ]
    }

    /// Where `at` is, a step on, in a text `len` long whose edited ranges are `ranges`, the
    /// last edit having started at `last`.
    fn resolve(at: At, step: isize, ranges: &[Range<usize>], last: usize, len: usize) -> usize {
        let pick = |index: usize| ranges.get(index % ranges.len().max(1));
        let place = match at {
            At::RangeStart(index) => pick(index).map_or(0, |range| range.start),
            At::RangeEnd(index) => pick(index).map_or(len, |range| range.end),
            At::LastEdit => last,
            At::Start => 0,
            At::End => len,
            At::Anywhere(at) => at % (len + 1),
        };
        place.saturating_add_signed(step).min(len)
    }

    /// `raw` as edits ascending and apart in a text `len` long.
    fn edits_from(
        raw: &[RawEdit],
        ranges: &[Range<usize>],
        last: usize,
        len: usize,
    ) -> Vec<OffsetEdit> {
        let mut edits: Vec<OffsetEdit> = Vec::new();
        let mut from = 0;
        for edit_at in raw {
            let start = resolve(edit_at.at, edit_at.step, ranges, last, len).max(from);
            if start > len {
                break;
            }
            let old_len = match edit_at.old {
                Old::Short(old) => old.min(len - start),
                Old::Rest => len - start,
            };
            edits.push(edit(start, old_len, edit_at.new));
            from = start + old_len.max(1);
        }
        edits
    }

    /// `points` within a text `len` long, sorted, as ranges in pairs; they may touch or repeat.
    fn ranges_of(mut points: Vec<usize>, len: usize) -> Vec<Range<usize>> {
        points.iter_mut().for_each(|at| *at %= len + 1);
        points.sort_unstable();
        points
            .chunks(2)
            .map(|pair| pair[0]..pair.get(1).copied().unwrap_or(pair[0]))
            .collect()
    }

    proptest::proptest! {
        /// As [`EditedLines`] uses them: the buffer's set follows every batch, covers spans and
        /// takes ranges in; a set a save took only follows the batches, until it is given
        /// back; and so does a set a cleanup command tracks, which may touch or repeat.
        #[test]
        fn the_gap_keeps_the_ranges_one_list_keeps(
            len in 0..400usize,
            ops in proptest::collection::vec(op(), 1..60),
        ) {
            let mut len = len;
            let mut last = 0;
            let (mut now, mut plain) = (Edited::default(), Plain::default());
            let (mut taken, mut plain_taken) = (Edited::default(), Plain::default());
            let (mut tracked, mut plain_tracked) = (Edited::default(), Vec::new());
            for op in ops {
                match op {
                    Op::Follow(raw) => {
                        let edits = edits_from(&raw, &plain.0, last, len);
                        now.follow(&edits);
                        plain.follow(&edits);
                        taken.follow_only(&edits);
                        follow_ranges(&mut plain_taken.0, &edits);
                        tracked.follow_only(&edits);
                        follow_ranges(&mut plain_tracked, &edits);
                        last = edits.last().map_or(last, |edit| edit.start);
                        len = edits.iter().fold(len, |len, e| len + e.new_len - e.old_len);
                    }
                    Op::Cover(at, span) => {
                        let start = resolve(at, 0, &plain.0, last, len);
                        let span = start..(start + span).min(len);
                        now.cover(span.clone());
                        plain.cover(span);
                    }
                    Op::Add(points) => {
                        let ranges = ranges_of(points, len);
                        now.add(ranges.iter().cloned());
                        plain.merge(ranges.into_iter());
                    }
                    Op::Take(points) => {
                        // As `Buffer::save_took_edits` does.
                        let mut kept = ranges_of(points, len);
                        join_touching(&mut kept);
                        taken = std::mem::replace(&mut now, Edited::from_ranges(kept.clone()));
                        plain_taken = std::mem::replace(&mut plain, Plain(kept));
                    }
                    Op::GiveBack => {
                        now.add(std::mem::take(&mut taken).ranges());
                        plain.merge(std::mem::take(&mut plain_taken.0).into_iter());
                    }
                    Op::Track(points) => {
                        plain_tracked = ranges_of(points, len);
                        tracked = Edited::from_ranges(plain_tracked.clone());
                    }
                    Op::Clone => {
                        (now, taken, tracked) = (now.clone(), taken.clone(), tracked.clone());
                    }
                    Op::Clear => {
                        now.clear();
                        plain.0.clear();
                    }
                }
                proptest::prop_assert_eq!(all(&now), plain.0.clone());
                proptest::prop_assert_eq!(now.len(), plain.0.len());
                proptest::prop_assert_eq!(all(&taken), plain_taken.0.clone());
                proptest::prop_assert_eq!(all(&tracked), plain_tracked.clone());
            }
        }
    }

    #[test]
    fn edits_one_after_another_move_only_the_ranges_near_the_gap() {
        // A forward replay: one edit after another, each far from the last range.
        let mut edited = Edited::default();
        for line in 0..10_000 {
            edited.follow(&[edit(line * 10, 0, 1)]);
            assert!(edited.scratch.len() <= 1, "line {line}");
        }
        assert_eq!(edited.len(), 10_000);
        assert_eq!(edited.ranges().nth(1), Some(10..11));
        // A backward replay: one edit before another, each passing one range.
        for line in (0..10_000).rev() {
            edited.follow(&[edit(line * 10 + 5, 0, 1)]);
            assert!(edited.scratch.is_empty(), "line {line}");
            assert_eq!(edited.before.len(), line + 2, "line {line}");
        }
        assert_eq!(edited.len(), 20_000);
        // Typing back at the start moves the gap there, then it stays.
        edited.follow(&[edit(1, 0, 1)]);
        assert_eq!(edited.before.len(), 1);
        assert_eq!(
            edited.ranges().take(3).collect::<Vec<_>>(),
            [0..2, 6..7, 12..13]
        );
    }
}
