//! Applying a batch of replacements to a rope and its display map, and the batch
//! that undoes it (ADR 0053).
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only text.

use std::borrow::Cow;
use std::ops::Range;

use super::Replacement;
use crate::display_map::DisplayMap;
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
    apply_marking(splices, text, display, &mut Vec::new())
}

/// As [`apply`], and sets `changed` to where the text may now break into display
/// clusters otherwise than before: the chunks the splices rebuilt, in the offsets
/// after the whole batch, ascending and apart. A cursor or mark anywhere else is
/// still where a cursor can be (ADR 0091).
pub(super) fn apply_marking<T: SpliceText>(
    splices: &[Splice<T>],
    text: &mut Rope,
    display: &mut DisplayMap,
    changed: &mut Vec<Range<usize>>,
) -> Batch {
    let mut undo: Batch = Vec::with_capacity(splices.len());
    // Where each splice's rebuilt chunks start before the splices below it apply,
    // and where they are once all have.
    let mut rebuilt = Vec::with_capacity(splices.len());
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
        display.sync(text, &edit);
        let start = splice.start - old_below + new_below;
        let before = splice.start.saturating_sub(edit.new.start);
        let after = edit.new.end.saturating_sub(splice.start + new_len);
        rebuilt.push((
            edit.new.start,
            start.saturating_sub(before)..start + new_len + after,
        ));
        undo.push(Splice {
            start,
            len: new_len,
            text: removed,
        });
    }
    undo.reverse();
    rebuilt.reverse();
    changed.clear();
    for (index, (first, range)) in rebuilt.into_iter().enumerate() {
        // Chunks that reach back into the splice below were rebuilt again by it, so the
        // two count as one run from where that one's starts.
        let joins = index
            .checked_sub(1)
            .is_some_and(|below| first <= splices[below].start + splices[below].len);
        match changed.last_mut() {
            Some(last) if joins || range.start <= last.end => last.end = last.end.max(range.end),
            _ => changed.push(range),
        }
    }
    undo
}

/// Applies the batches of an undo group, last first, as an undo or a redo crosses it.
/// Returns what cursors follow, batch by batch, the batches that undo them, and where
/// [`apply_marking`] says clusters may have changed, once all have applied.
pub(super) fn apply_group(
    group: &[Batch],
    text: &mut Rope,
    display: &mut DisplayMap,
) -> (Vec<Vec<OffsetEdit>>, Vec<Batch>, Vec<Range<usize>>) {
    let mut steps = Vec::with_capacity(group.len());
    let mut batches = Vec::with_capacity(group.len());
    let (mut changed, mut more) = (Vec::new(), Vec::new());
    for batch in group.iter().rev() {
        let step: Vec<_> = batch.iter().map(offset_edit).collect();
        // What earlier batches changed moves with this one.
        follow_ranges(&mut changed, &step);
        steps.push(step);
        batches.push(apply_marking(batch, text, display, &mut more));
        changed = union(changed, std::mem::take(&mut more));
    }
    (steps, batches, changed)
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
