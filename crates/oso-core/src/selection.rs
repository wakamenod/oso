//! Selection sets: a window's cursors and marks, whether their regions show, and
//! the ring of earlier marks (ADR 0007, ADR 0040, ADR 0071).
//!
//! Must not depend on: `buffer`, `motion` or `window`; it holds offsets and a
//! goal column, and `motion` decides how they move.
//!
//! A window has one or more selections. Commands take the whole set and
//! move every selection, so multiple cursors (M2) need no new command
//! signatures. Each selection has its own mark, and the set says whether the
//! regions between them show (`transient-mark-mode`), for all of them at once.
//!
//! Cursors, marks and the ring follow edits here, as offsets
//! ([`SelectionSet::follow_edits`], ADR 0053). The editor then clips them to the
//! display clusters of the new text.

use std::ops::Range;

use crate::sum_tree::Bias;

/// Where vertical motion (`C-n`, `C-p`) aims, kept across consecutive
/// vertical moves so that short lines do not pull the cursor left for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Goal {
    /// A display column, in cells.
    Column(usize),
    /// The end of each line: vertical motion started at a line end
    /// (`track-eol`, ADR 0030).
    LineEnd,
}

/// One selection: a cursor, its mark, and the goal of a vertical move in
/// progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Selection {
    /// Point: a buffer offset at the start of a display cluster.
    pub cursor: usize,
    /// The other end of the region, also at the start of a display cluster, once
    /// a mark has been set. It stays when the region is hidden.
    pub mark: Option<usize>,
    /// Set by vertical motion and kept by the next one; every other motion
    /// clears it.
    pub goal: Option<Goal>,
}

impl Selection {
    /// A selection with its cursor at `offset`, no mark and no goal.
    pub const fn at(offset: usize) -> Self {
        Self {
            cursor: offset,
            mark: None,
            goal: None,
        }
    }

    /// Moves the cursor to `offset` with no goal, and keeps the mark.
    pub fn move_to(&mut self, offset: usize) {
        self.cursor = offset;
        self.goal = None;
    }

    /// The text between the mark and the cursor, in either order. Empty, at the
    /// cursor, with no mark.
    pub fn range(&self) -> Range<usize> {
        let mark = self.mark.unwrap_or(self.cursor);
        mark.min(self.cursor)..mark.max(self.cursor)
    }
}

/// Whether a window's regions show, for all its selections at once
/// (`transient-mark-mode`, ADR 0071).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MarkActive {
    /// Hidden. The marks stay, for `C-x C-x` and `C-u C-SPC`.
    #[default]
    Inactive,
    /// Shown by `C-SPC`, `C-x C-x` or `C-x h`, until an edit, `C-g`, `C-SPC C-SPC`
    /// or another buffer in the window.
    Active,
    /// Shown by a motion with Shift. The next motion without it hides it.
    Shifted,
}

impl MarkActive {
    /// Whether the regions show.
    pub fn is_active(self) -> bool {
        self != Self::Inactive
    }
}

/// The most marks a [`MarkRing`] keeps, as Emacs's `mark-ring-max`.
pub const MARK_RING_SIZE: usize = 16;

/// The primary cursor's earlier marks, the newest first, which `C-u C-SPC` and
/// `C-,` go back through. They follow edits as the marks do. It holds no more
/// than [`MARK_RING_SIZE`], and the oldest goes first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MarkRing {
    marks: [usize; MARK_RING_SIZE],
    len: usize,
}

impl MarkRing {
    /// The marks, the newest first.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = usize> + '_ {
        self.marks[..self.len].iter().copied()
    }

    /// Puts `mark` first, dropping the oldest if the ring is full.
    pub(crate) fn push(&mut self, mark: usize) {
        self.len = (self.len + 1).min(MARK_RING_SIZE);
        self.marks[..self.len].rotate_right(1);
        self.marks[0] = mark;
    }

    /// Takes the newest mark and puts `current` last, as Emacs's `pop-mark` does,
    /// so that going on goes round the ring. `None`, and nothing changes, if the
    /// ring is empty.
    pub(crate) fn rotate(&mut self, current: usize) -> Option<usize> {
        let newest = *self.marks[..self.len].first()?;
        self.marks[..self.len].rotate_left(1);
        self.marks[self.len - 1] = current;
        Some(newest)
    }

    /// Moves every mark through `edits`, as [`SelectionSet::follow_edits`] moves
    /// marks. They go through in ascending order, on the stack.
    pub(crate) fn follow_edits(&mut self, edits: &[OffsetEdit]) {
        let mut order = [(0, 0); MARK_RING_SIZE];
        let order = &mut order[..self.len];
        for (index, slot) in order.iter_mut().enumerate() {
            *slot = (self.marks[index], index);
        }
        order.sort_unstable();
        let mut marks = Follow::new(edits, Bias::Left);
        for &(mark, index) in order.iter() {
            self.marks[index] = marks.map(mark);
        }
    }

    fn update(&mut self, mut f: impl FnMut(usize) -> usize) {
        for mark in &mut self.marks[..self.len] {
            *mark = f(*mark);
        }
    }
}

/// A window's selections: never empty, sorted by cursor, with no two
/// cursors at one offset, and while the region shows, no two regions that
/// overlap. One of them is primary: the one the window scrolls to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionSet {
    selections: Vec<Selection>,
    primary: usize,
    mark_active: MarkActive,
    ring: MarkRing,
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self::new(0)
    }
}

/// One replacement as a cursor sees it: `old_len` bytes at `start` became
/// `new_len` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OffsetEdit {
    pub(crate) start: usize,
    pub(crate) old_len: usize,
    pub(crate) new_len: usize,
}

/// Where new text ends after `edits`, ascending. The edits are as in
/// [`SelectionSet::follow_edits`], and deletions have no new text.
pub(crate) fn new_text_ends(edits: &[OffsetEdit]) -> impl Iterator<Item = usize> + '_ {
    let (mut old_end, mut new_end) = (0, 0);
    edits.iter().filter_map(move |edit| {
        new_end += edit.start - old_end + edit.new_len;
        old_end = edit.start + edit.old_len;
        (edit.new_len > 0).then_some(new_end)
    })
}

/// Moves offsets through sorted edits, as [`SelectionSet::follow_edits`] says. The
/// offsets must come in ascending order, and cost O(offsets + edits) together.
struct Follow<'a> {
    edits: &'a [OffsetEdit],
    bias: Bias,
    /// Where the edits passed so far end, before and after them.
    old_end: usize,
    new_end: usize,
    next: usize,
}

impl<'a> Follow<'a> {
    fn new(edits: &'a [OffsetEdit], bias: Bias) -> Self {
        Self {
            edits,
            bias,
            old_end: 0,
            new_end: 0,
            next: 0,
        }
    }

    fn map(&mut self, offset: usize) -> usize {
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

/// Sorts a set when it goes out of scope, also when a panic unwinds past it.
struct KeepOrder<'a> {
    set: &'a mut SelectionSet,
    changed: bool,
}

impl Drop for KeepOrder<'_> {
    fn drop(&mut self) {
        if self.changed {
            self.set.normalize();
        }
    }
}

/// What [`SelectionSet::update`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct UpdateOutcome<E> {
    /// Some cursor or mark moved. A changed goal alone shows nothing.
    pub changed: bool,
    /// The first error a selection reported. Every selection was still
    /// updated, as far as it could go.
    pub error: Option<E>,
}

impl SelectionSet {
    /// One cursor at `offset`.
    pub fn new(offset: usize) -> Self {
        Self {
            selections: vec![Selection::at(offset)],
            primary: 0,
            mark_active: MarkActive::Inactive,
            ring: MarkRing::default(),
        }
    }

    /// The set of `selections`, with the one at position `primary` primary,
    /// sorted and with those at one offset merged, and the region hidden. `None`
    /// if there is no such position, which also rules out an empty set.
    pub fn from_parts(selections: Vec<Selection>, primary: usize) -> Option<Self> {
        (primary < selections.len()).then(|| {
            let mut set = Self {
                selections,
                primary,
                ..Self::new(0)
            };
            set.normalize();
            set
        })
    }

    /// The primary selection.
    pub fn primary(&self) -> &Selection {
        &self.selections[self.primary]
    }

    /// The position of the primary selection in [`SelectionSet::iter`].
    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// All selections, in buffer order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Selection> {
        self.selections.iter()
    }

    /// All selections, in buffer order, to search.
    pub fn as_slice(&self) -> &[Selection] {
        &self.selections
    }

    /// The number of selections; at least 1.
    pub fn len(&self) -> usize {
        self.selections.len()
    }

    /// Always false: a set has at least one selection.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Whether the regions show.
    pub fn mark_active(&self) -> MarkActive {
        self.mark_active
    }

    /// Shows or hides the regions. Shown, those that overlap merge. Returns
    /// whether it was shown or hidden before.
    pub fn set_mark_active(&mut self, active: MarkActive) -> MarkActive {
        let before = std::mem::replace(&mut self.mark_active, active);
        if active.is_active() && !before.is_active() {
            self.normalize();
        }
        before
    }

    /// The primary cursor's earlier marks.
    pub fn mark_ring(&self) -> &MarkRing {
        &self.ring
    }

    /// Sets the mark of every selection at its cursor, and puts the primary's
    /// old mark, if it had one, first in the ring, as Emacs's `push-mark` does.
    /// The region stays as it was.
    pub fn push_marks(&mut self) {
        if let Some(mark) = self.primary().mark {
            self.ring.push(mark);
        }
        for selection in &mut self.selections {
            selection.mark = Some(selection.cursor);
        }
    }

    /// Sets the primary selection's mark at `offset`, and puts its old mark, if it had
    /// one, first in the ring, as Emacs's `push-mark` does for one cursor. The others keep
    /// theirs, and the region stays as it was.
    pub fn push_primary_mark(&mut self, offset: usize) {
        let primary = &mut self.selections[self.primary];
        if let Some(mark) = primary.mark.replace(offset) {
            self.ring.push(mark);
        }
    }

    /// Moves the primary cursor to `offset` with no goal, and keeps its mark. The others
    /// stay, and merge with it as [`SelectionSet::update`] merges, if it meets them.
    pub fn move_primary(&mut self, offset: usize) {
        let order = KeepOrder {
            set: self,
            changed: true,
        };
        let primary = order.set.primary;
        order.set.selections[primary].move_to(offset);
    }

    /// Keeps only the primary selection, and the ring.
    pub fn keep_primary(&mut self) {
        let primary = *self.primary();
        self.selections = vec![primary];
        self.primary = 0;
    }

    /// Keeps only the primary selection, with its cursor at its mark, and the mark
    /// going back through the ring, as Emacs's `pop-to-mark-command` does. The
    /// region hides. `false`, and nothing changes, if the primary has no mark.
    pub fn pop_mark(&mut self) -> bool {
        let Some(mark) = self.primary().mark else {
            return false;
        };
        let previous = self.ring.rotate(mark).unwrap_or(mark);
        self.keep_primary();
        self.selections[0] = Selection {
            cursor: mark,
            mark: Some(previous),
            goal: None,
        };
        self.mark_active = MarkActive::Inactive;
        true
    }

    /// Runs `f` on every selection, then merges selections whose cursors
    /// met, or while the region shows, whose regions overlap. A selection that
    /// fails still keeps what `f` changed, as an Emacs motion that hits the
    /// buffer end moves as far as it can. If `f` panics, the set is still put in
    /// order, so that a panicking command leaves a valid set.
    pub fn update<E>(
        &mut self,
        mut f: impl FnMut(&mut Selection) -> Result<(), E>,
    ) -> UpdateOutcome<E> {
        let mut changed = false;
        let mut error = None;
        // Assumes a change until every selection has run.
        let mut order = KeepOrder {
            set: self,
            changed: true,
        };
        for selection in &mut order.set.selections {
            let before = (selection.cursor, selection.mark);
            if let Err(e) = f(selection) {
                error.get_or_insert(e);
            }
            changed |= (selection.cursor, selection.mark) != before;
        }
        order.changed = changed;
        drop(order);
        UpdateOutcome { changed, error }
    }

    /// Moves every cursor, mark and mark in the ring through `edits`, which are
    /// sorted, do not overlap and are all in the offsets before them. An edit is
    /// not a vertical move, so goals are cleared. O(cursors + edits), and the marks
    /// O(marks log marks) more when they are out of order.
    ///
    /// An offset before an edit stays, and one after it shifts. For a cursor,
    /// the `bias` decides the rest:
    ///
    /// - `Bias::Right`, for the window that edits: a cursor at the start of an
    ///   edit, or inside its replaced text, ends after the new text.
    /// - `Bias::Left`, for other windows, as an Emacs marker: a cursor at the
    ///   start of an edit stays before the new text, and one inside the
    ///   replaced text moves there too.
    ///
    /// With either, a cursor at the end of the replaced text stays before the
    /// text that followed it. Marks always follow as Emacs markers do, with
    /// `Bias::Left`, so text typed at a mark goes after it.
    pub(crate) fn follow_edits(&mut self, edits: &[OffsetEdit], bias: Bias) {
        self.follow(edits, bias, Bias::Left);
    }

    /// Moves everything through `edits` as [`SelectionSet::follow_edits`] does, but a
    /// mark at its cursor moves as the cursor does, so an empty region stays empty. For
    /// a message logged where the cursors are, which is no edit of theirs.
    pub(crate) fn follow_logged(&mut self, edits: &[OffsetEdit], bias: Bias) {
        self.follow(edits, bias, bias);
    }

    fn follow(&mut self, edits: &[OffsetEdit], bias: Bias, at_cursor: Bias) {
        follow_marks(&mut self.selections, edits, at_cursor);
        let mut cursors = Follow::new(edits, bias);
        self.update(|selection| {
            selection.cursor = cursors.map(selection.cursor);
            selection.goal = None;
            Ok::<(), ()>(())
        });
        self.ring.follow_edits(edits);
    }

    /// Runs `f` on every mark in the ring, for the editor to clip them to a
    /// display cluster.
    pub(crate) fn update_ring(&mut self, f: impl FnMut(usize) -> usize) {
        self.ring.update(f);
    }

    /// This set with `ring` in place of its own, for an undo, which puts back
    /// cursors and marks but not the ring.
    pub(crate) fn with_ring(&self, ring: MarkRing) -> Self {
        Self {
            ring,
            ..self.clone()
        }
    }

    /// Sorts the selections and merges those at one offset, or while the region
    /// shows, those whose regions overlap, keeping the primary one.
    fn normalize(&mut self) {
        let primary = self.selections[self.primary];
        if !self.mark_active.is_active() {
            // As before marks: cursors that met merge, and the primary keeps its own.
            self.selections.sort_by_key(|selection| selection.cursor);
            self.selections.dedup_by(|next, kept| {
                let met = next.cursor == kept.cursor;
                if met && *next == primary {
                    *kept = *next;
                }
                met
            });
            self.primary = self
                .selections
                .partition_point(|selection| selection.cursor < primary.cursor);
            return;
        }
        self.normalize_regions(primary);
    }

    /// [`SelectionSet::normalize`] while the regions show: those that overlap merge.
    fn normalize_regions(&mut self, primary: Selection) {
        self.selections
            .sort_by_key(|selection| (selection.range().start, selection.cursor));
        let mut primary_at = self
            .selections
            .iter()
            .position(|selection| *selection == primary)
            .unwrap_or(0);
        let mut kept = 0;
        for index in 1..self.selections.len() {
            let (last, next) = (self.selections[kept], self.selections[index]);
            let overlap = next.range().start < last.range().end;
            if next.cursor == last.cursor || overlap {
                let keep_next = index == primary_at;
                self.selections[kept] = merge(last, next, keep_next);
            } else {
                kept += 1;
                self.selections[kept] = next;
            }
            if index == primary_at {
                primary_at = kept;
            }
            // A merged region's cursor can go back to its start, onto an empty
            // selection kept just before it.
            while kept > 0 && self.selections[kept - 1].cursor == self.selections[kept].cursor {
                let (last, next) = (self.selections[kept - 1], self.selections[kept]);
                self.selections[kept - 1] = merge(last, next, primary_at == kept);
                if primary_at == kept {
                    primary_at -= 1;
                }
                kept -= 1;
            }
        }
        self.selections.truncate(kept + 1);
        self.primary = primary_at;
        debug_assert!(
            self.selections
                .is_sorted_by_key(|selection| selection.cursor)
        );
    }
}

/// Moves the marks of `selections` through `edits` as markers, in ascending order.
/// Marks set at the cursors are already in order and go through in place. Others,
/// such as those of regions in both directions, are sorted first, which allocates.
/// A mark at its cursor moves with `at_cursor` instead. Each bias maps its own marks,
/// still in ascending order.
fn follow_marks(selections: &mut [Selection], edits: &[OffsetEdit], at_cursor: Bias) {
    let mut others = Follow::new(edits, Bias::Left);
    let mut along = Follow::new(edits, at_cursor);
    let mut map = |selection: &Selection, mark: usize| {
        if mark == selection.cursor {
            along.map(mark)
        } else {
            others.map(mark)
        }
    };
    let in_order = selections.iter().filter_map(|selection| selection.mark);
    if in_order.is_sorted() {
        for selection in selections {
            selection.mark = selection.mark.map(|mark| map(selection, mark));
        }
        return;
    }
    let mut order: Vec<_> = (selections.iter().enumerate())
        .filter_map(|(index, selection)| Some((selection.mark?, index)))
        .collect();
    order.sort_unstable();
    for (mark, index) in order {
        selections[index].mark = Some(map(&selections[index], mark));
    }
}

/// One selection for two whose regions overlap or whose cursors met, while the
/// regions show: the union of their regions. It keeps the direction, mark and goal
/// of `next` if `keep_next`, and of `last` otherwise.
fn merge(last: Selection, next: Selection, keep_next: bool) -> Selection {
    let kept = if keep_next { next } else { last };
    let (a, b) = (last.range(), next.range());
    let union = a.start.min(b.start)..a.end.max(b.end);
    if union.is_empty() {
        return kept;
    }
    let forward = kept.mark.is_none_or(|mark| mark <= kept.cursor);
    let (cursor, mark) = if forward {
        (union.end, union.start)
    } else {
        (union.start, union.end)
    };
    Selection {
        cursor,
        mark: Some(mark),
        goal: kept.goal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_knows_its_primary_and_is_never_empty() {
        let set = SelectionSet::from_parts(vec![Selection::at(3), Selection::at(9)], 1).unwrap();
        assert_eq!(set.primary_index(), 1);
        assert_eq!(set.primary().cursor, 9);
        assert_eq!(set.len(), 2);
        assert!(!set.is_empty());
    }

    #[test]
    fn update_reports_changes_and_the_first_error() {
        let mut set = SelectionSet::new(3);
        let update = set.update(|s| {
            s.cursor += 1;
            Err::<(), _>("End of buffer")
        });
        assert_eq!(
            update,
            UpdateOutcome {
                changed: true,
                error: Some("End of buffer")
            }
        );
        assert_eq!(set.primary().cursor, 4);
        let update = set.update(|_| Ok::<(), ()>(()));
        assert!(!update.changed);
    }

    #[test]
    fn from_parts_sorts_merges_and_keeps_the_primary() {
        let parts = vec![Selection::at(9), Selection::at(1), Selection::at(9)];
        let set = SelectionSet::from_parts(parts.clone(), 1).unwrap();
        assert_eq!(set.iter().map(|s| s.cursor).collect::<Vec<_>>(), [1, 9]);
        assert_eq!(set.primary().cursor, 1);
        assert_eq!(set.primary_index(), 0);
        assert_eq!(SelectionSet::from_parts(parts, 3), None);
        assert_eq!(SelectionSet::from_parts(Vec::new(), 0), None);
    }

    #[test]
    fn cursors_that_meet_merge_and_the_primary_survives() {
        let mut set = SelectionSet {
            selections: vec![Selection::at(1), Selection::at(5), Selection::at(9)],
            primary: 2,
            ..SelectionSet::new(0)
        };
        set.update(|s| {
            s.cursor = s.cursor.min(5);
            Ok::<(), ()>(())
        });
        assert_eq!(set.len(), 2);
        assert_eq!(set.primary().cursor, 5);
        assert_eq!(
            set.iter().map(|s| s.cursor).collect::<Vec<_>>(),
            [1, 5],
            "sorted and merged"
        );
    }

    fn set_at(cursors: &[usize]) -> SelectionSet {
        let parts = cursors.iter().copied().map(Selection::at).collect();
        SelectionSet::from_parts(parts, 0).unwrap()
    }

    fn followed(cursors: &[usize], edits: &[(usize, usize, usize)], bias: Bias) -> Vec<usize> {
        let edits: Vec<OffsetEdit> = edits
            .iter()
            .map(|&(start, old_len, new_len)| OffsetEdit {
                start,
                old_len,
                new_len,
            })
            .collect();
        let mut set = set_at(cursors);
        set.follow_edits(&edits, bias);
        set.iter().map(|selection| selection.cursor).collect()
    }

    #[test]
    fn a_cursor_at_an_insertion_goes_after_it_only_in_the_editing_window() {
        assert_eq!(followed(&[5], &[(5, 0, 3)], Bias::Right), [8]);
        assert_eq!(followed(&[5], &[(5, 0, 3)], Bias::Left), [5]);
        assert_eq!(followed(&[4], &[(5, 0, 3)], Bias::Right), [4], "before it");
        assert_eq!(followed(&[6], &[(5, 0, 3)], Bias::Left), [9], "after it");
    }

    #[test]
    fn a_cursor_in_replaced_text_ends_by_the_bias() {
        // Bytes 2..6 became 3 bytes, ending at 5.
        let edit = [(2, 4, 3)];
        assert_eq!(followed(&[2], &edit, Bias::Right), [5], "at the start");
        assert_eq!(followed(&[2], &edit, Bias::Left), [2], "at the start");
        assert_eq!(followed(&[3], &edit, Bias::Right), [5], "inside");
        assert_eq!(followed(&[5], &edit, Bias::Left), [2], "inside");
        assert_eq!(followed(&[6], &edit, Bias::Right), [5], "at the end");
        assert_eq!(followed(&[6], &edit, Bias::Left), [5], "at the end");
        assert_eq!(followed(&[7], &edit, Bias::Left), [6], "past the end");
    }

    #[test]
    fn many_cursors_shift_by_every_edit_before_them() {
        // Typing "xy" at each of three cursors, as a window with three does.
        let edits = [(1, 0, 2), (4, 0, 2), (9, 0, 2)];
        assert_eq!(followed(&[1, 4, 9], &edits, Bias::Right), [3, 8, 15]);
        assert_eq!(followed(&[1, 4, 9], &edits, Bias::Left), [1, 6, 13]);
        // Deleting shifts back, and a cursor between edits by what came before.
        let edits = [(1, 2, 0), (6, 1, 0)];
        assert_eq!(followed(&[0, 5, 8], &edits, Bias::Left), [0, 3, 5]);
    }

    #[test]
    fn adjacent_edits_are_passed_in_turn() {
        // 2..4 replaced, then an insertion right where it ends.
        let edits = [(2, 2, 1), (4, 0, 5)];
        assert_eq!(followed(&[4], &edits, Bias::Right), [8]);
        assert_eq!(followed(&[4], &edits, Bias::Left), [3]);
    }

    #[test]
    fn following_merges_cursors_clears_goals_and_keeps_the_primary() {
        let parts = vec![
            Selection {
                cursor: 2,
                mark: None,
                goal: Some(Goal::LineEnd),
            },
            Selection::at(4),
            Selection::at(9),
        ];
        let mut set = SelectionSet::from_parts(parts, 2).unwrap();
        set.follow_edits(
            &[OffsetEdit {
                start: 1,
                old_len: 4,
                new_len: 0,
            }],
            Bias::Right,
        );
        assert_eq!(set.iter().map(|s| s.cursor).collect::<Vec<_>>(), [1, 5]);
        assert_eq!(set.primary().cursor, 5);
        assert!(set.iter().all(|selection| selection.goal.is_none()));
    }

    #[test]
    fn a_panic_in_an_update_leaves_a_valid_set() {
        let mut set = set_at(&[1, 5, 9]);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            set.update(|selection| {
                selection.cursor = 20 - selection.cursor;
                assert!(selection.cursor > 15, "a bug in a command");
                Ok::<(), ()>(())
            })
        }));
        assert!(panicked.is_err());
        let cursors: Vec<_> = set.iter().map(|selection| selection.cursor).collect();
        assert_eq!(cursors, [9, 15, 19], "the two it reached, and the last");
        assert_eq!(set.primary().cursor, 19, "the primary, wherever it went");
        assert_eq!(set.primary_index(), 2);
    }

    #[test]
    fn new_text_ends_are_in_the_offsets_after_the_edits() {
        let edit = |start, old_len, new_len| OffsetEdit {
            start,
            old_len,
            new_len,
        };
        let edits = [edit(1, 0, 2), edit(4, 3, 0), edit(9, 1, 5), edit(10, 0, 1)];
        let ends: Vec<_> = new_text_ends(&edits).collect();
        assert_eq!(ends, [3, 13, 14]);
        assert_eq!(new_text_ends(&[]).count(), 0);
    }

    #[test]
    fn no_edits_only_clear_goals() {
        assert_eq!(followed(&[3, 7], &[], Bias::Right), [3, 7]);
    }

    fn region(cursor: usize, mark: usize) -> Selection {
        Selection {
            mark: Some(mark),
            ..Selection::at(cursor)
        }
    }

    fn shown(selections: Vec<Selection>, primary: usize) -> SelectionSet {
        let mut set = SelectionSet::from_parts(selections, primary).unwrap();
        set.set_mark_active(MarkActive::Active);
        set
    }

    #[test]
    fn the_ring_keeps_the_newest_sixteen_and_goes_round() {
        let mut ring = MarkRing::default();
        assert_eq!(ring.rotate(9), None, "empty");
        assert_eq!(ring.iter().len(), 0);
        for mark in 0..20 {
            ring.push(mark);
        }
        assert_eq!(ring.iter().len(), MARK_RING_SIZE);
        assert_eq!(ring.iter().next(), Some(19));
        assert_eq!(ring.iter().last(), Some(4), "the oldest went first");

        let mut ring = MarkRing::default();
        ring.push(1);
        ring.push(2);
        assert_eq!(ring.rotate(3), Some(2));
        assert_eq!(ring.iter().collect::<Vec<_>>(), [1, 3]);
        assert_eq!(ring.rotate(2), Some(1));
        assert_eq!(ring.rotate(1), Some(3), "round again");
    }

    #[test]
    fn pushing_marks_sets_one_at_every_cursor_and_keeps_the_primarys_old_one() {
        let mut set = set_at(&[2, 6]);
        set.push_marks();
        assert_eq!(set.mark_ring().iter().len(), 0, "no mark before");
        assert!(
            set.iter()
                .all(|selection| selection.mark == Some(selection.cursor))
        );
        set.update(|selection| {
            selection.cursor += 1;
            Ok::<(), ()>(())
        });
        set.push_marks();
        assert_eq!(set.mark_ring().iter().collect::<Vec<_>>(), [2]);
        assert_eq!(set.primary().mark, Some(3));
        assert_eq!(
            set.mark_active(),
            MarkActive::Inactive,
            "pushing shows nothing"
        );
    }

    #[test]
    fn popping_goes_to_the_mark_with_one_cursor_and_hides_the_region() {
        let mut set = set_at(&[1, 8]);
        assert!(!set.pop_mark(), "no mark");
        set.push_marks();
        set.update(|selection| {
            selection.cursor += 2;
            Ok::<(), ()>(())
        });
        set.push_marks();
        set.set_mark_active(MarkActive::Active);
        assert!(set.pop_mark());
        assert_eq!(set.len(), 1);
        assert_eq!(
            *set.primary(),
            region(3, 1),
            "at the mark, with the older one"
        );
        assert_eq!(set.mark_active(), MarkActive::Inactive);
        assert_eq!(set.mark_ring().iter().collect::<Vec<_>>(), [3]);
    }

    #[test]
    fn marks_follow_edits_as_markers_whatever_the_cursors_bias() {
        // Text typed at a mark goes after it, and a mark in deleted text goes to
        // its start.
        for bias in [Bias::Left, Bias::Right] {
            let mut set = SelectionSet::from_parts(vec![region(9, 4)], 0).unwrap();
            set.follow_edits(
                &[OffsetEdit {
                    start: 4,
                    old_len: 0,
                    new_len: 2,
                }],
                bias,
            );
            assert_eq!(set.primary().mark, Some(4));
            set.follow_edits(
                &[OffsetEdit {
                    start: 2,
                    old_len: 4,
                    new_len: 1,
                }],
                bias,
            );
            assert_eq!(set.primary().mark, Some(2));
        }
    }

    #[test]
    fn marks_out_of_order_still_follow_every_edit() {
        let mut set = SelectionSet::from_parts(vec![region(1, 9), region(5, 0)], 0).unwrap();
        set.push_marks();
        let edits = [
            OffsetEdit {
                start: 0,
                old_len: 0,
                new_len: 1,
            },
            OffsetEdit {
                start: 7,
                old_len: 0,
                new_len: 1,
            },
        ];
        let mut marks = SelectionSet::from_parts(vec![region(1, 9), region(5, 0)], 0).unwrap();
        marks.follow_edits(&edits, Bias::Left);
        let after: Vec<_> = marks.iter().map(|s| s.mark).collect();
        assert_eq!(after, [Some(11), Some(0)], "9 after both, 0 before both");
        set.follow_edits(&edits, Bias::Left);
        assert_eq!(
            set.mark_ring().iter().collect::<Vec<_>>(),
            [11],
            "as its mark"
        );
    }

    #[test]
    fn a_mark_out_of_order_at_an_insertion_already_passed_stays_before_it() {
        // Marks 10 then 8, and 2 bytes typed at 8: the 8 stays, the 10 shifts.
        let mut set = SelectionSet::from_parts(vec![region(1, 10), region(2, 8)], 0).unwrap();
        let typed = [OffsetEdit {
            start: 8,
            old_len: 0,
            new_len: 2,
        }];
        set.follow_edits(&typed, Bias::Left);
        let marks: Vec<_> = set.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [Some(12), Some(8)]);
        // The ring holds marks in any order too.
        let mut ring = MarkRing::default();
        ring.push(4);
        ring.push(7);
        ring.follow_edits(&[OffsetEdit {
            start: 4,
            old_len: 0,
            new_len: 2,
        }]);
        assert_eq!(ring.iter().collect::<Vec<_>>(), [9, 4]);
    }

    #[test]
    fn shown_regions_that_overlap_merge_and_keep_the_primarys_direction() {
        // 2..6 forward and 4..9 backward, the second primary.
        let set = shown(vec![region(6, 2), region(4, 9)], 1);
        assert_eq!(set.iter().copied().collect::<Vec<_>>(), [region(2, 9)]);
        assert_eq!(set.primary_index(), 0);
        let set = shown(vec![region(6, 2), region(4, 9)], 0);
        assert_eq!(*set.primary(), region(9, 2), "forward, as the first");
        // A cursor inside another region joins it.
        let set = shown(vec![region(8, 2), Selection::at(5)], 1);
        assert_eq!(*set.primary(), region(8, 2));
    }

    #[test]
    fn hidden_regions_and_regions_that_only_touch_stay_apart() {
        let hidden = SelectionSet::from_parts(vec![region(6, 2), region(4, 9)], 0).unwrap();
        assert_eq!(hidden.len(), 2);
        let touching = shown(vec![region(4, 2), region(9, 4)], 0);
        assert_eq!(touching.len(), 2);
        let cursors: Vec<_> = touching.iter().map(|s| s.cursor).collect();
        assert_eq!(cursors, [4, 9]);
        // Touching at both cursors is one cursor.
        let mut met = shown(vec![region(4, 2), region(5, 9)], 0);
        met.update(|selection| {
            selection.cursor = selection.cursor.min(4);
            Ok::<(), ()>(())
        });
        assert_eq!(met.iter().copied().collect::<Vec<_>>(), [region(9, 2)]);
    }

    #[test]
    fn showing_the_region_merges_and_hiding_it_does_nothing_more() {
        let mut set = SelectionSet::from_parts(vec![region(6, 2), region(4, 9)], 0).unwrap();
        assert_eq!(
            set.set_mark_active(MarkActive::Shifted),
            MarkActive::Inactive
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.set_mark_active(MarkActive::Inactive),
            MarkActive::Shifted
        );
        assert_eq!(set.len(), 1);
    }

    fn arb_selection() -> impl Strategy<Value = Selection> {
        (0usize..40, proptest::option::of(0usize..40)).prop_map(|(cursor, mark)| Selection {
            mark,
            ..Selection::at(cursor)
        })
    }

    use proptest::prelude::*;

    /// Sorted edits that do not overlap, as `(start, old_len, new_len)` in old offsets.
    fn arb_edits() -> impl Strategy<Value = Vec<OffsetEdit>> {
        proptest::collection::vec((0usize..6, 0usize..4, 0usize..4), 0..6).prop_map(|steps| {
            let mut at = 0;
            steps
                .into_iter()
                .map(|(gap, old_len, new_len)| {
                    let edit = OffsetEdit {
                        start: at + gap,
                        old_len,
                        new_len,
                    };
                    at = edit.start + old_len;
                    edit
                })
                .collect()
        })
    }

    proptest! {
        #[test]
        fn prop_marks_in_any_order_follow_as_each_would_alone(
            marks in proptest::collection::vec(0usize..40, 1..10),
            edits in arb_edits(),
        ) {
            let alone = |mark| Follow::new(&edits, Bias::Left).map(mark);
            let parts = (marks.iter().enumerate())
                .map(|(cursor, &mark)| Selection { mark: Some(mark), ..Selection::at(cursor * 100) })
                .collect();
            let mut set = SelectionSet::from_parts(parts, 0).unwrap();
            let mut ring = MarkRing::default();
            for &mark in marks.iter().take(MARK_RING_SIZE) {
                ring.push(mark);
            }
            let ring_before: Vec<_> = ring.iter().collect();
            set.follow_edits(&edits, Bias::Left);
            ring.follow_edits(&edits);
            let followed: Vec<_> = set.iter().map(|s| s.mark.unwrap()).collect();
            let expected: Vec<_> = marks.iter().map(|&mark| alone(mark)).collect();
            prop_assert_eq!(followed, expected);
            let ring_expected: Vec<_> = ring_before.into_iter().map(alone).collect();
            prop_assert_eq!(ring.iter().collect::<Vec<_>>(), ring_expected);
        }

        #[test]
        fn prop_shown_regions_end_sorted_apart_and_the_primary_kept(
            selections in proptest::collection::vec(arb_selection(), 1..12),
            primary in 0usize..12,
            moves in proptest::collection::vec(-5isize..6, 12),
        ) {
            let primary = primary % selections.len();
            let wanted = selections[primary];
            let mut set = shown(selections.clone(), primary);
            let covered = |set: &SelectionSet, offset: usize| {
                set.iter().any(|s| (s.range().start..=s.range().end).contains(&offset))
            };
            for selection in &selections {
                prop_assert!(covered(&set, selection.cursor));
            }
            prop_assert!(set.primary().range().start <= wanted.range().start);
            prop_assert!(set.primary().range().end >= wanted.range().end);
            let mut moves = moves.into_iter();
            set.update(|selection| {
                let step = moves.next().unwrap_or(0);
                selection.cursor = selection.cursor.saturating_add_signed(step);
                Ok::<(), ()>(())
            });
            let all: Vec<_> = set.iter().copied().collect();
            for pair in all.windows(2) {
                prop_assert!(pair[0].cursor < pair[1].cursor, "sorted by cursor");
                prop_assert!(pair[0].range().end <= pair[1].range().start, "apart");
            }
            prop_assert!(set.primary_index() < set.len());
        }
    }
}
