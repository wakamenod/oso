//! Applying a batch of replacements to a rope and its display map, and the batch
//! that undoes it (ADR 0053).
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only text.

use std::borrow::Cow;
use std::ops::Range;

use super::Replacement;
use crate::display_map::DisplayMap;
use crate::display_map::wrap_map::Splice as WrapSplice;
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
/// order they apply, for the windows' wraps to follow (ADR 0102).
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
        let before = wraps.is_some().then(|| display.snapshot());
        display.sync(text, &edit);
        if let (Some(wraps), Some(before)) = (wraps.as_deref_mut(), before) {
            wraps.push(WrapSplice {
                before,
                after: display.snapshot(),
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

/// Applies the batches of an undo group, last first, as an undo or a redo crosses it.
/// Returns what cursors follow, batch by batch, the batches that undo them, and where
/// [`apply_marking`] says clusters may have changed, once all have applied.
pub(super) fn apply_group(
    group: &[Batch],
    text: &mut Rope,
    display: &mut DisplayMap,
    mut wraps: Option<&mut Vec<WrapSplice>>,
) -> (Vec<Vec<OffsetEdit>>, Vec<Batch>, Vec<Range<usize>>) {
    let mut steps = Vec::with_capacity(group.len());
    let mut batches = Vec::with_capacity(group.len());
    let (mut changed, mut more) = (Vec::new(), Vec::new());
    for batch in group.iter().rev() {
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
mod tests {
    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::*;
    use crate::display_map::TabWidth;
    use crate::rope::tests::{ATOMS, floor_boundary, text_strategy};

    /// Each chunk of `rope`, with where it starts.
    fn chunks(rope: &Rope) -> Vec<(usize, String)> {
        let mut at = 0;
        rope.chunk_texts(0..rope.len())
            .map(|text| {
                let start = at;
                at += text.len();
                (start, text.to_string())
            })
            .collect()
    }

    /// Splices from `raw`, at character boundaries of `text`, sorted and apart.
    fn batch<'a>(text: &str, raw: Vec<(usize, usize, &'a str)>) -> Vec<Splice<Cow<'a, str>>> {
        let mut cut: Vec<(usize, usize, &str)> = raw
            .into_iter()
            .map(|(at, len, new)| {
                let start = floor_boundary(text, at);
                (start, floor_boundary(text, start + len), new)
            })
            .collect();
        cut.sort_by_key(|&(start, end, _)| (start, end));
        let mut splices: Vec<Splice<Cow<'a, str>>> = Vec::new();
        for (start, end, new) in cut {
            let apart = splices
                .last()
                .is_none_or(|last| start > last.start + last.len);
            if apart && (start < end || !new.is_empty()) {
                splices.push(Splice {
                    start,
                    len: end - start,
                    text: Cow::Borrowed(new),
                });
            }
        }
        splices
    }

    /// Where `offset`, outside every splice, is once they have applied; `None` inside one,
    /// or at its end if `inclusive`.
    fn moved<T: SpliceText>(
        splices: &[Splice<T>],
        offset: usize,
        inclusive: bool,
    ) -> Option<usize> {
        let mut delta = 0isize;
        for splice in splices {
            let (start, end) = (splice.start, splice.start + splice.len);
            if offset < start {
                break;
            }
            let after = if inclusive {
                offset > end
            } else {
                offset >= end
            };
            if !after {
                return None;
            }
            delta += splice.text.byte_len() as isize - splice.len as isize;
        }
        offset.checked_add_signed(delta)
    }

    /// Checks that every chunk the batch rebuilt is in `changed`.
    fn rebuilt_chunks_are_changed(text: &str, splices: &[Splice<Cow<'_, str>>]) {
        let mut rope = Rope::from(text);
        let kept: HashSet<(usize, String)> = chunks(&rope)
            .into_iter()
            .filter_map(|(start, chunk)| {
                let end = start + chunk.len();
                let outside = splices
                    .iter()
                    .all(|splice| end <= splice.start || start >= splice.start + splice.len);
                outside
                    .then(|| moved(splices, start, false))
                    .flatten()
                    .map(|start| (start, chunk))
            })
            .collect();
        let mut display = DisplayMap::new(&rope, TabWidth::new(4));
        let mut changed = Vec::new();
        apply_marking(splices, &mut rope, &mut display, &mut changed, None);
        for (start, chunk) in chunks(&rope) {
            if kept.contains(&(start, chunk.clone())) {
                continue;
            }
            let end = start + chunk.len();
            assert!(
                changed
                    .iter()
                    .any(|range| range.start <= start && end <= range.end),
                "chunk {start}..{end} is not in {changed:?}"
            );
        }
    }

    #[test]
    fn chunks_rebuilt_back_past_the_splice_below_are_changed() {
        let text = "aのaのaa👍\r\na👍\r\n\r\n\u{7f}\r\n\u{e0041}🇯🇵\r\n日本語aaa\r\na\u{200b}\r\n\r\nの\r\nの\r\na日本語\u{e0041}の👨\u{200d}👩\u{200d}👧日本語a";
        rebuilt_chunks_are_changed(text, &batch(text, vec![(95, 0, "a"), (109, 0, "a")]));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Every chunk a batch rebuilt is in what it says changed (ADR 0091).
        #[test]
        fn every_rebuilt_chunk_is_changed(
            text in text_strategy(60),
            raw in prop::collection::vec((0usize..400, 0usize..6, prop::sample::select(ATOMS)), 1..6),
        ) {
            let splices = batch(&text, raw);
            prop_assume!(!splices.is_empty());
            rebuilt_chunks_are_changed(&text, &splices);
        }

        /// A cluster start outside every splice and outside what changed is still one,
        /// also among long runs of marks and of regional indicators.
        #[test]
        fn what_did_not_change_still_starts_its_clusters(
            text in prop::collection::vec(
                prop_oneof![
                    3 => prop::sample::select(ATOMS).prop_map(str::to_owned),
                    3 => (10usize..60).prop_map(|n| format!("a{}", "\u{301}".repeat(n))),
                    1 => (40usize..160).prop_map(|n| "\u{1f1ef}".repeat(n)),
                ],
                0..30,
            ).prop_map(|atoms| atoms.concat()),
            raw in prop::collection::vec((0usize..1200, 0usize..6, prop::sample::select(ATOMS)), 1..8),
        ) {
            let splices = batch(&text, raw);
            let mut rope = Rope::from(text.as_str());
            let mut display = DisplayMap::new(&rope, TabWidth::new(4));
            let before = display.snapshot();
            let starts: Vec<usize> = (0..=text.len())
                .filter(|&at| text.is_char_boundary(at) && before.cluster_at(at).0 == at)
                .filter_map(|at| moved(&splices, at, true))
                .collect();
            let mut changed = Vec::new();
            apply_marking(&splices, &mut rope, &mut display, &mut changed, None);
            let after = display.snapshot();
            for at in starts {
                let index = changed.partition_point(|range| range.end < at);
                if changed.get(index).is_some_and(|range| range.start <= at) {
                    continue;
                }
                prop_assert_eq!(after.cluster_at(at).0, at, "changed {:?}", changed);
            }
        }
    }
}
