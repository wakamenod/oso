//! Applying a batch of replacements to a rope and its display map, and the batch
//! that undoes it (ADR 0053). An undo group applies a slice at a time in `sliced`
//! (ADR 0119).
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only text.

use std::borrow::Cow;
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
        sync(text, display, &edit, wraps.as_deref_mut());
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
    settle_wraps(display, wraps);
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

/// Brings `display` along with `text` after `edit`. With `wraps`, the splice goes there
/// too, with the display before and after it; past [`SPLICES_READ`] of them, with the
/// snapshot from there as a placeholder, as [`apply_marking`] says.
pub(super) fn sync(
    text: &Rope,
    display: &mut DisplayMap,
    edit: &RopeEdit,
    wraps: Option<&mut Vec<WrapSplice>>,
) {
    let read = wraps
        .as_ref()
        .is_some_and(|wraps| wraps.len() < SPLICES_READ);
    let before = read.then(|| display.snapshot());
    display.sync(text, edit);
    if let Some(wraps) = wraps {
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
}

/// Makes the last splice's display the real one, whatever stood in before it.
pub(super) fn settle_wraps(display: &DisplayMap, wraps: Option<&mut Vec<WrapSplice>>) {
    if let Some(last) = wraps.and_then(|wraps| wraps.last_mut()) {
        last.after = display.snapshot();
    }
}

/// The union of two lists of ranges that are each ascending and apart, also ascending
/// and apart. O(ranges).
pub(super) fn union(a: Vec<Range<usize>>, b: Vec<Range<usize>>) -> Vec<Range<usize>> {
    if a.is_empty() {
        return b;
    }
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(a.len() + b.len());
    let (mut a, mut b) = (a.into_iter().peekable(), b.into_iter().peekable());
    while let Some(range) = match (a.peek(), b.peek()) {
        (Some(x), Some(y)) if y.start < x.start => b.next(),
        (Some(_), _) => a.next(),
        (None, _) => b.next(),
    } {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
pub(super) mod tests;
