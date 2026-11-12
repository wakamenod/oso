//! Applying a batch of replacements to a rope and its display map, and the batch
//! that undoes it (ADR 0053). An undo group's batches that lie apart apply as one
//! (ADR 0119).
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only text.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::Range;

use super::Replacement;
use crate::display_map::DisplayMap;
use crate::display_map::wrap_map::{SPLICES_READ, Splice as WrapSplice};
use crate::rope::{Rope, RopeEdit};
use crate::selection::{OffsetEdit, follow_ranges};
use crate::undo::{Batch, Splice};

/// The new text of a [`Splice`]: borrowed for an edit, a rope or nothing for undo
/// and redo.
pub(super) trait SpliceText {
    fn byte_len(&self) -> usize;
    /// Replaces `range` of `rope` with this text.
    fn replace_in(&self, rope: &mut Rope, range: Range<usize>) -> RopeEdit;
}

impl SpliceText for Cow<'_, str> {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn replace_in(&self, rope: &mut Rope, range: Range<usize>) -> RopeEdit {
        rope.replace(range, self)
    }
}

impl SpliceText for Option<Rope> {
    fn byte_len(&self) -> usize {
        self.as_ref().map_or(0, Rope::len)
    }

    fn replace_in(&self, rope: &mut Rope, range: Range<usize>) -> RopeEdit {
        match self {
            Some(text) => rope.replace(range, &text.to_string()),
            None => rope.replace(range, ""),
        }
    }
}

pub(super) fn offset_edit<T: SpliceText>(splice: &Splice<T>) -> OffsetEdit {
    OffsetEdit {
        start: splice.start,
        old_len: splice.len,
        new_len: splice.text.byte_len(),
    }
}

/// Clamps `edits`, drops empty ones, sorts them and merges those
/// that overlap, so that the result is one batch. Ranges that overlap, and
/// insertions at one offset, become their union, replaced by their texts joined
/// in offset order. Edits with one start keep their given order. Ranges that
/// only touch stay apart.
pub(super) fn normalized<'a>(text: &Rope, edits: &[Replacement<'a>]) -> Vec<Splice<Cow<'a, str>>> {
    let mut sorted: Vec<Splice<Cow<'a, str>>> = edits
        .iter()
        .map(|edit| {
            let (start, end) = text.clip_range(edit.range.clone());
            Splice {
                start,
                len: end - start,
                text: Cow::Borrowed(edit.text),
            }
        })
        .filter(|splice| splice.len > 0 || !splice.text.is_empty())
        .collect();
    sorted.sort_by_key(|splice| (splice.start, splice.start + splice.len));
    sorted.dedup_by(|next, last| {
        let merges = overlap(last, next);
        if merges {
            let end = (last.start + last.len).max(next.start + next.len);
            last.len = end - last.start;
            last.text.to_mut().push_str(&next.text);
        }
        merges
    });
    sorted
}

/// Whether `next`, which does not start before `last`, overlaps it.
fn overlap<T>(last: &Splice<T>, next: &Splice<T>) -> bool {
    next.start < last.start + last.len
        || (last.len == 0 && next.len == 0 && last.start == next.start)
}

/// Applies one batch to `text` and `display`, in descending offset order so
/// that each splice leaves the offsets below it valid. Returns the batch that
/// undoes it.
#[cfg(test)]
pub(super) fn apply<T: SpliceText>(
    splices: &[Splice<T>],
    text: &mut Rope,
    display: &mut DisplayMap,
) -> Batch {
    apply_marking(splices, text, display, &mut Vec::new(), None)
}

/// As [`apply`], and sets `changed` to where the text may now break into display
/// clusters otherwise than before: the chunks the splices rebuilt, in the offsets
/// after the whole batch, ascending and apart. A cursor or mark anywhere else is
/// still where a cursor can be (ADR 0091).
///
/// With `wraps`, each splice also goes there with the display before and after it, in the
/// order they apply, for the windows' wraps to follow (ADR 0102). Past [`SPLICES_READ`] of
/// them, which is as many as a rewrap reads, each holds the snapshot from there as a
/// placeholder, and only the last one's `after` is the display after it, so that a thousand
/// cursors keep no copy of the display map's path for each (ADR 0103).
pub(super) fn apply_marking<T: SpliceText>(
    splices: &[Splice<T>],
    text: &mut Rope,
    display: &mut DisplayMap,
    changed: &mut Vec<Range<usize>>,
    mut wraps: Option<&mut Vec<WrapSplice>>,
) -> Batch {
    let mut undo: Batch = Vec::with_capacity(splices.len());
    // Where each splice's rebuilt chunks start, in the offsets before the batch, as the
    // splices below it have not applied yet, and where they end once all have.
    let mut starts = Vec::with_capacity(splices.len());
    let mut ends = Vec::with_capacity(splices.len());
    // What the splices below the current one take, before and after them.
    let mut old_below: usize = splices.iter().map(|splice| splice.len).sum();
    let mut new_below: usize = splices.iter().map(|s| s.text.byte_len()).sum();
    for splice in splices.iter().rev() {
        let new_len = splice.text.byte_len();
        old_below -= splice.len;
        new_below -= new_len;
        let range = splice.start..splice.start + splice.len;
        let removed = (!range.is_empty()).then(|| text.slice(range.clone()));
        let edit = splice.text.replace_in(text, range);
        let read = wraps
            .as_ref()
            .is_some_and(|wraps| wraps.len() < SPLICES_READ);
        let before = read.then(|| display.snapshot());
        display.sync(text, &edit);
        if let Some(wraps) = wraps.as_deref_mut() {
            let (before, after) = match (before, wraps.last()) {
                (Some(before), _) => (before, display.snapshot()),
                (None, Some(last)) => (last.after.clone(), last.after.clone()),
                (None, None) => (display.snapshot(), display.snapshot()),
            };
            wraps.push(WrapSplice {
                before,
                after,
                edit: edit.clone(),
            });
        }
        let start = splice.start - old_below + new_below;
        let after = edit.new.end.saturating_sub(splice.start + new_len);
        starts.push(edit.new.start..edit.new.start);
        ends.push(start + new_len + after);
        undo.push(Splice {
            start,
            len: new_len,
            text: removed,
        });
    }
    undo.reverse();
    // The last one's display is the real one, whatever stood in before it.
    if let Some(last) = wraps.and_then(|wraps| wraps.last_mut()) {
        last.after = display.snapshot();
    }
    // A splice's chunks can reach back past those of the splices below it, so their
    // starts go through the whole batch, in order.
    let mut order: Vec<usize> = (0..starts.len()).collect();
    order.sort_unstable_by_key(|&index| starts[index].start);
    let mut sorted: Vec<Range<usize>> = order.iter().map(|&index| starts[index].clone()).collect();
    let edits: Vec<OffsetEdit> = splices.iter().map(offset_edit).collect();
    follow_ranges(&mut sorted, &edits);
    let mut rebuilt: Vec<Range<usize>> = order
        .iter()
        .zip(sorted)
        .map(|(&index, start)| start.start..ends[index])
        .collect();
    rebuilt.sort_unstable_by_key(|range| range.start);
    changed.clear();
    for range in rebuilt {
        match changed.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => changed.push(range),
        }
    }
    undo
}

/// Applies the batches of an undo group, last first, as an undo or a redo crosses it, those
/// that lie apart as one ([`joined_apart`]). Returns what cursors follow and the batches that
/// undo them, one of each a batch applied, so not one a batch of `group`, and where
/// [`apply_marking`] says clusters may have changed, once all have applied.
pub(super) fn apply_group(
    group: &[Batch],
    text: &mut Rope,
    display: &mut DisplayMap,
    mut wraps: Option<&mut Vec<WrapSplice>>,
) -> (Vec<Vec<OffsetEdit>>, Vec<Batch>, Vec<Range<usize>>) {
    let runs = joined_apart(group);
    let mut steps = Vec::with_capacity(runs.len());
    let mut batches = Vec::with_capacity(runs.len());
    let (mut changed, mut more) = (Vec::new(), Vec::new());
    for batch in &runs {
        let step: Vec<_> = batch.iter().map(offset_edit).collect();
        // What earlier batches changed moves with this one.
        follow_ranges(&mut changed, &step);
        steps.push(step);
        batches.push(apply_marking(
            batch,
            text,
            display,
            &mut more,
            wraps.as_deref_mut(),
        ));
        changed = union(changed, std::mem::take(&mut more));
    }
    (steps, batches, changed)
}

/// The batches of `group` in the order they apply, last first, with those that lie apart
/// joined: a batch whose splices all lie before those joined so far, or all after what they
/// became, with a byte between, joins them, since one batch then does what they did one after
/// another. So a step of many small batches, as a query-replace session answered match by match
/// makes, applies, and is mapped and followed, as one batch rather than one an answer, whose
/// cost would each grow with the ranges the batches before it changed (ADR 0119). O(splices).
fn joined_apart(group: &[Batch]) -> Vec<Cow<'_, [Splice<Option<Rope>>]>> {
    let mut runs = Vec::new();
    // The run being joined: borrowed while it is one batch.
    let mut alone: Option<&[Splice<Option<Rope>>]> = None;
    let mut joined: VecDeque<Splice<Option<Rope>>> = VecDeque::new();
    // What the run adds to the length, so that a batch after it can be put in the offsets
    // before it.
    let mut grown: isize = 0;
    for batch in group.iter().rev() {
        let (Some(first), Some(last)) = (batch.first(), batch.last()) else {
            continue;
        };
        let front = alone.and_then(<[_]>::first).or(joined.front());
        let back = alone.and_then(<[_]>::last).or(joined.back());
        let before = front.is_some_and(|front| last.start + last.len < front.start);
        let after = back.is_some_and(|back| {
            (back.start + back.len)
                .checked_add_signed(grown)
                .is_some_and(|end| first.start > end)
        });
        if before || after {
            if let Some(alone) = alone.take() {
                joined.extend(alone.iter().cloned());
            }
            if before {
                batch
                    .iter()
                    .rev()
                    .for_each(|splice| joined.push_front(splice.clone()));
            } else {
                // Each starts past where the run ends after it, so past where it ends before.
                joined.extend(batch.iter().map(|splice| Splice {
                    start: splice.start.saturating_add_signed(-grown),
                    len: splice.len,
                    text: splice.text.clone(),
                }));
            }
        } else {
            end_run(&mut runs, &mut alone, &mut joined);
            alone = Some(batch);
            grown = 0;
        }
        grown += batch
            .iter()
            .map(|splice| splice.text.byte_len().cast_signed() - splice.len.cast_signed())
            .sum::<isize>();
    }
    end_run(&mut runs, &mut alone, &mut joined);
    runs
}

/// Puts the run being joined in `runs`, if there is one.
fn end_run<'a>(
    runs: &mut Vec<Cow<'a, [Splice<Option<Rope>>]>>,
    alone: &mut Option<&'a [Splice<Option<Rope>>]>,
    joined: &mut VecDeque<Splice<Option<Rope>>>,
) {
    if let Some(alone) = alone.take() {
        runs.push(Cow::Borrowed(alone));
    } else if !joined.is_empty() {
        runs.push(Cow::Owned(Vec::from(std::mem::take(joined))));
    }
}

/// The union of two lists of ranges that are each ascending and apart, also ascending
/// and apart.
fn union(a: Vec<Range<usize>>, b: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut all = a;
    all.extend(b);
    all.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(all.len());
    for range in all {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests;
