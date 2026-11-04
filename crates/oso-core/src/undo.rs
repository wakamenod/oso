//! Undo history: linear undo and redo of one buffer's edits (ADR 0030,
//! ADR 0053).
//!
//! Must not depend on: `buffer`, `motion`, `window` or `editor`; it stores what
//! the buffer did and applies nothing.
//!
//! A [`Group`] holds one command's edits, across all cursors. It stores what
//! to do to go back, so undo and redo are one operation: crossing a group
//! applies its splices and yields the group that crosses back. Removed text
//! is kept as rope slices, which share chunks with the rope they came from,
//! and inserted text costs nothing until a group is crossed.
//!
//! An edit after an undo discards the redo groups. The history is bounded in
//! bytes, not counting the group last recorded or crossed, which always stays
//! whatever its size. Redo groups go first, from the furthest, then the oldest
//! undo groups. A buffer whose saved state was dropped from the history
//! can never get that state's [`ContentId`] back, so it stays modified until
//! it is saved.

use std::collections::VecDeque;
use std::mem;

use crate::rope::Rope;
use crate::selection::{Selection, SelectionSet};

/// The bytes a buffer's history keeps at most, apart from its newest group.
const DEFAULT_LIMIT: usize = 64 * 1024 * 1024;

/// How many consecutive commands join one group, as Emacs's
/// `amalgamating-undo-limit`, so that undoing long typing takes a few steps.
const JOIN_LIMIT: usize = 20;

/// Identifies a state of a buffer's content. Each edit gets a new id, and undo
/// and redo restore the id of the state they return to. Ids are never reused,
/// so two states with one id have the same text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContentId(pub(crate) u64);

/// `len` bytes at `start` become `text`. The splices of one batch are sorted,
/// do not overlap and are all in the offsets before the batch.
#[derive(Clone)]
pub(crate) struct Splice<T> {
    pub(crate) start: usize,
    pub(crate) len: usize,
    pub(crate) text: T,
}

/// The splices one edit call made, or what undoes them. A splice that only
/// deletes has no text, so typing keeps no empty rope per cursor.
pub(crate) type Batch = Vec<Splice<Option<Rope>>>;

/// What a group returns to on one of its sides.
#[derive(Clone, Debug)]
pub(crate) struct Side {
    pub(crate) content: ContentId,
    /// The cursors of the window that edited, if the selected window showed
    /// the buffer.
    pub(crate) selections: Option<SelectionSet>,
}

impl Side {
    fn cost(&self) -> usize {
        self.selections.as_ref().map_or(0, |set| {
            mem::size_of::<SelectionSet>() + set.len() * mem::size_of::<Selection>()
        })
    }
}

/// One step of undo or redo. Neither this nor [`Splice`] has a `Debug`, so that
/// their text cannot reach a log.
#[derive(Clone)]
pub(crate) struct Group {
    /// Applied from the last batch to the first, they lead to `target`.
    pub(crate) batches: Vec<Batch>,
    /// The side this group leads to when crossed.
    pub(crate) target: Side,
    /// The side it leaves when crossed.
    pub(crate) source: Side,
    /// The commands that joined it.
    joined: usize,
    /// What the history counts for it.
    bytes: usize,
}

/// What a splice costs the history, besides any text.
const SPLICE_COST: usize = mem::size_of::<Splice<Option<Rope>>>();

/// What a rope holding text costs the history, besides the text: an estimate of
/// its tree node.
const ROPE_COST: usize = 128;

fn batch_cost(batch: &Batch) -> usize {
    batch
        .iter()
        .map(|splice| {
            SPLICE_COST
                + splice
                    .text
                    .as_ref()
                    .map_or(0, |text| ROPE_COST + text.len())
        })
        .sum::<usize>()
        + mem::size_of::<Batch>()
}

impl Group {
    fn new(batches: Vec<Batch>, target: Side, source: Side) -> Self {
        let bytes = mem::size_of::<Self>()
            + batches.iter().map(batch_cost).sum::<usize>()
            + target.cost()
            + source.cost();
        Self {
            batches,
            target,
            source,
            joined: 0,
            bytes,
        }
    }

    /// The group that crosses back, once `batches` undid this one's.
    pub(crate) fn crossed(&self, batches: Vec<Batch>) -> Self {
        Self::new(batches, self.source.clone(), self.target.clone())
    }
}

/// How a recorded edit relates to the newest group.
#[derive(Debug)]
pub(crate) enum Opening {
    /// It starts a group, which returns to these cursors.
    New(Option<SelectionSet>),
    /// It belongs to the command that is writing the newest group.
    Continue,
    /// It starts a command that joins the newest group.
    Join,
}

/// Which way to go through the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Undo,
    Redo,
}

/// A buffer's undo and redo groups. See the module docs.
pub(crate) struct History {
    /// Oldest first.
    undo: VecDeque<Group>,
    /// The next one to redo last.
    redo: VecDeque<Group>,
    /// The cost of every group in both.
    bytes: usize,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_LIMIT)
    }
}

impl History {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }

    /// Forgets everything, for a buffer whose content was replaced.
    pub(crate) fn clear(&mut self) {
        *self = Self::new(self.limit);
    }

    /// Records an edit that led from `content` to `after`, discarding the
    /// redo groups. `batch` undoes it.
    pub(crate) fn record(
        &mut self,
        batch: Batch,
        opening: Opening,
        content: ContentId,
        after: Side,
    ) {
        self.bytes -= self.redo.iter().map(|group| group.bytes).sum::<usize>();
        self.redo.clear();
        let cost = batch_cost(&batch);
        let newest = match opening {
            Opening::New(_) => None,
            Opening::Continue | Opening::Join => self.undo.back_mut(),
        };
        if let Some(group) = newest {
            let (old, new) = (group.source.cost(), after.cost() + cost);
            group.batches.push(batch);
            group.source = after;
            group.bytes = group.bytes + new - old;
            group.joined += usize::from(matches!(opening, Opening::Join));
            self.bytes = self.bytes + new - old;
        } else {
            let selections = match opening {
                Opening::New(selections) => selections,
                Opening::Continue | Opening::Join => None,
            };
            let target = Side {
                content,
                selections,
            };
            let group = Group::new(vec![batch], target, after);
            self.bytes += group.bytes;
            self.undo.push_back(group);
        }
        self.evict(Direction::Undo);
    }

    /// Sets the cursors the newest group ends with, once its command is done.
    pub(crate) fn set_after(&mut self, selections: SelectionSet) {
        if let Some(group) = self.undo.back_mut() {
            let before = group.source.cost();
            group.source.selections = Some(selections);
            let after = group.source.cost();
            group.bytes = group.bytes + after - before;
            self.bytes = self.bytes + after - before;
        }
    }

    /// Whether a command that starts at `content` with cursors `selections`
    /// continues where the newest group ended, so it may join it.
    pub(crate) fn can_join(&self, content: ContentId, selections: Option<&SelectionSet>) -> bool {
        self.redo.is_empty()
            && self.undo.back().is_some_and(|group| {
                group.joined + 1 < JOIN_LIMIT
                    && group.source.content == content
                    && group.source.selections.as_ref() == selections
            })
    }

    /// The group an undo or a redo would cross.
    pub(crate) fn next(&self, direction: Direction) -> Option<&Group> {
        match direction {
            Direction::Undo => self.undo.back(),
            Direction::Redo => self.redo.back(),
        }
    }

    /// Moves the group crossed to the other stack, as `crossed`.
    pub(crate) fn cross(&mut self, direction: Direction, crossed: Group) {
        let left = match direction {
            Direction::Undo => self.undo.pop_back(),
            Direction::Redo => self.redo.pop_back(),
        };
        self.bytes -= left.map_or(0, |group| group.bytes);
        self.bytes += crossed.bytes;
        // The group crossed lands on the other stack, and must stay.
        let newest = match direction {
            Direction::Undo => {
                self.redo.push_back(crossed);
                Direction::Redo
            }
            Direction::Redo => {
                self.undo.push_back(crossed);
                Direction::Undo
            }
        };
        self.evict(newest);
    }

    /// Drops the redo groups furthest away, then the oldest undo groups, until
    /// all but the newest group, last on the `newest` stack, fit the limit.
    fn evict(&mut self, newest: Direction) {
        // A newest undo group is safe: while it alone is left, nothing is over.
        let redo_kept = usize::from(newest == Direction::Redo);
        let newest_bytes = self.next(newest).map_or(0, |group| group.bytes);
        while self.bytes - newest_bytes > self.limit {
            let dropped = if self.redo.len() > redo_kept {
                self.redo.pop_front()
            } else {
                self.undo.pop_front()
            };
            let Some(dropped) = dropped else {
                break;
            };
            self.bytes -= dropped.bytes;
        }
    }

    #[cfg(test)]
    pub(crate) fn lens(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(content: u64) -> Side {
        Side {
            content: ContentId(content),
            selections: None,
        }
    }

    fn splice(start: usize, len: usize, text: &str) -> Batch {
        vec![Splice {
            start,
            len,
            text: (!text.is_empty()).then(|| Rope::from(text)),
        }]
    }

    /// The cost of a batch of one splice of `bytes` text.
    fn one_batch(bytes: usize) -> usize {
        let text = if bytes == 0 { 0 } else { ROPE_COST + bytes };
        SPLICE_COST + text + mem::size_of::<Batch>()
    }

    /// The cost of a group with one such batch and no cursors.
    fn group_cost(bytes: usize) -> usize {
        mem::size_of::<Group>() + one_batch(bytes)
    }

    fn recompute(history: &History) -> usize {
        history
            .undo
            .iter()
            .chain(&history.redo)
            .map(|group| {
                mem::size_of::<Group>()
                    + group.batches.iter().map(batch_cost).sum::<usize>()
                    + group.target.cost()
                    + group.source.cost()
            })
            .sum()
    }

    fn record(history: &mut History, opening: Opening, from: u64, to: u64, text: &str) {
        history.record(splice(0, 0, text), opening, ContentId(from), side(to));
        assert_eq!(history.bytes(), recompute(history), "the count stays exact");
    }

    #[test]
    fn a_splice_with_no_text_costs_no_rope() {
        assert_eq!(
            batch_cost(&splice(0, 1, "")),
            SPLICE_COST + mem::size_of::<Batch>()
        );
        assert_eq!(
            batch_cost(&splice(0, 1, "abc")),
            SPLICE_COST + ROPE_COST + 3 + mem::size_of::<Batch>()
        );
    }

    #[test]
    fn a_batch_costs_its_text_and_its_splices() {
        let mut history = History::default();
        record(&mut history, Opening::New(None), 0, 1, "abc");
        assert_eq!(history.bytes(), group_cost(3));
        record(&mut history, Opening::Continue, 1, 2, "de");
        assert_eq!(history.bytes(), group_cost(3) + one_batch(2));
        assert_eq!(history.lens(), (1, 0));
    }

    #[test]
    fn cursors_count_toward_the_bound() {
        let set = SelectionSet::from_parts(vec![Selection::at(1), Selection::at(2)], 0).unwrap();
        let side = Side {
            content: ContentId(0),
            selections: Some(set.clone()),
        };
        assert_eq!(
            side.cost(),
            mem::size_of::<SelectionSet>() + 2 * mem::size_of::<Selection>()
        );

        let mut history = History::default();
        record(&mut history, Opening::New(Some(set.clone())), 0, 1, "");
        let with_before = history.bytes();
        assert_eq!(with_before, group_cost(0) + side.cost());
        history.set_after(set);
        assert_eq!(history.bytes(), with_before + side.cost());
        assert_eq!(history.bytes(), recompute(&history));
    }

    #[test]
    fn the_oldest_groups_go_first_and_the_newest_does_not_count() {
        let mut history = History::new(group_cost(10) * 2);
        record(&mut history, Opening::New(None), 0, 1, &"a".repeat(10));
        record(&mut history, Opening::New(None), 1, 2, &"b".repeat(10));
        record(&mut history, Opening::New(None), 2, 3, &"c".repeat(1000));
        assert_eq!(history.lens(), (3, 0), "the older ones fit exactly");
        assert_eq!(history.undo[2].target.content, ContentId(2));

        record(&mut history, Opening::New(None), 3, 4, "d");
        assert_eq!(history.lens(), (1, 0), "older than the newest, too big");
        assert_eq!(history.undo[0].target.content, ContentId(3));
    }

    #[test]
    fn one_byte_over_the_bound_drops_only_the_oldest() {
        let mut history = History::new(group_cost(10) + group_cost(11));
        record(&mut history, Opening::New(None), 0, 1, &"a".repeat(10));
        record(&mut history, Opening::New(None), 1, 2, &"b".repeat(11));
        record(&mut history, Opening::New(None), 2, 3, "");
        assert_eq!(history.lens(), (3, 0));
        record(&mut history, Opening::New(None), 3, 4, "");
        assert_eq!(history.lens(), (3, 0), "the oldest went");
        assert_eq!(history.undo[0].target.content, ContentId(1));
    }

    #[test]
    fn an_edit_discards_the_redo_groups_and_their_bytes() {
        let mut history = History::default();
        record(&mut history, Opening::New(None), 0, 1, "abc");
        let group = history.next(Direction::Undo).unwrap().clone();
        history.cross(Direction::Undo, group.crossed(vec![splice(0, 3, "xy")]));
        assert_eq!(history.lens(), (0, 1));
        assert_eq!(history.bytes(), group_cost(2));
        record(&mut history, Opening::New(None), 0, 2, "z");
        assert_eq!(history.lens(), (1, 0));
        assert_eq!(history.bytes(), group_cost(1));
    }

    #[test]
    fn crossing_swaps_the_sides_and_can_go_back() {
        let mut history = History::default();
        record(&mut history, Opening::New(None), 5, 6, "abc");
        let group = history.next(Direction::Undo).unwrap().clone();
        assert_eq!(group.target.content, ContentId(5));
        assert_eq!(group.source.content, ContentId(6));
        history.cross(Direction::Undo, group.crossed(vec![splice(0, 3, "")]));
        assert!(history.next(Direction::Undo).is_none());
        let group = history.next(Direction::Redo).unwrap().clone();
        assert_eq!(group.target.content, ContentId(6));
        assert_eq!(group.source.content, ContentId(5));
        history.cross(Direction::Redo, group.crossed(vec![splice(0, 0, "abc")]));
        assert_eq!(history.lens(), (1, 0));
        assert_eq!(history.bytes(), recompute(&history));
    }

    #[test]
    fn undoing_a_big_insertion_keeps_the_older_groups() {
        let mut history = History::new(group_cost(10));
        record(&mut history, Opening::New(None), 0, 1, &"a".repeat(10));
        record(&mut history, Opening::New(None), 1, 2, "");
        let group = history.next(Direction::Undo).unwrap().clone();
        // Undoing brings back what the group inserted: more than the bound.
        let big = splice(0, 0, &"b".repeat(1000));
        history.cross(Direction::Undo, group.crossed(vec![big]));
        assert_eq!(history.lens(), (1, 1));
    }

    #[test]
    fn redo_groups_go_before_undo_groups_from_the_furthest() {
        let mut history = History::default();
        for content in 0..4 {
            record(&mut history, Opening::New(None), content, content + 1, "");
        }
        for _ in 0..3 {
            let group = history.next(Direction::Undo).unwrap().clone();
            history.cross(Direction::Undo, group.crossed(vec![splice(0, 0, "abc")]));
        }
        assert_eq!(history.lens(), (1, 3));
        history.limit = group_cost(3) + group_cost(0);
        history.evict(Direction::Redo);
        assert_eq!(history.lens(), (1, 2), "the older undo group stays");
        assert_eq!(history.bytes(), recompute(&history));
        // The redo furthest away led to the newest state, 4.
        assert_eq!(history.redo[0].target.content, ContentId(3));
        assert_eq!(history.redo[1].target.content, ContentId(2));

        history.limit = 0;
        history.evict(Direction::Redo);
        assert_eq!(history.lens(), (0, 1), "at last only the newest group");
        assert_eq!(history.redo[0].target.content, ContentId(2));
        assert_eq!(history.bytes(), recompute(&history));
    }

    #[test]
    fn a_group_just_redone_stays_though_redo_groups_remain() {
        let mut history = History::default();
        record(&mut history, Opening::New(None), 0, 1, "");
        record(&mut history, Opening::New(None), 1, 2, "");
        for _ in 0..2 {
            let group = history.next(Direction::Undo).unwrap().clone();
            history.cross(Direction::Undo, group.crossed(vec![splice(0, 0, "")]));
        }
        history.limit = 0;
        let group = history.next(Direction::Redo).unwrap().clone();
        let big = splice(0, 0, &"a".repeat(1000));
        history.cross(Direction::Redo, group.crossed(vec![big]));
        assert_eq!(history.lens(), (1, 0), "the redo furthest away goes");
        assert_eq!(history.undo[0].target.content, ContentId(0));
    }

    #[test]
    fn a_group_can_be_joined_only_where_it_ended() {
        let set = SelectionSet::new(1);
        let mut history = History::default();
        assert!(!history.can_join(ContentId(0), None), "nothing to join");
        record(&mut history, Opening::New(None), 0, 1, "");
        history.set_after(set.clone());
        assert!(history.can_join(ContentId(1), Some(&set)));
        assert!(!history.can_join(ContentId(0), Some(&set)), "other text");
        assert!(!history.can_join(ContentId(1), None), "no cursors");
        assert!(
            !history.can_join(ContentId(1), Some(&SelectionSet::new(2))),
            "other cursors"
        );
    }

    #[test]
    fn a_pending_redo_stops_joining_though_the_group_before_it_fits() {
        let set = SelectionSet::new(1);
        let mut history = History::default();
        record(&mut history, Opening::New(None), 0, 1, "");
        history.set_after(set.clone());
        record(&mut history, Opening::New(None), 1, 2, "");
        let group = history.next(Direction::Undo).unwrap().clone();
        history.cross(Direction::Undo, group.crossed(vec![splice(0, 0, "")]));
        assert!(!history.can_join(ContentId(1), Some(&set)));
    }

    #[test]
    fn joining_stops_at_the_limit() {
        let set = SelectionSet::new(1);
        let mut history = History::default();
        record(&mut history, Opening::New(None), 0, 1, "");
        history.set_after(set.clone());
        for commands in 1..JOIN_LIMIT {
            assert!(history.can_join(ContentId(1), Some(&set)), "{commands}");
            record(&mut history, Opening::Join, 1, 1, "");
            history.set_after(set.clone());
        }
        assert!(!history.can_join(ContentId(1), Some(&set)));
        assert_eq!(history.lens(), (1, 0));
        assert_eq!(history.undo[0].batches.len(), JOIN_LIMIT);
    }

    #[test]
    fn continuing_with_no_group_starts_one() {
        let mut history = History::default();
        record(&mut history, Opening::Continue, 3, 4, "x");
        assert_eq!(history.lens(), (1, 0));
        assert_eq!(history.undo[0].target.content, ContentId(3));
    }

    #[test]
    fn clearing_keeps_the_limit() {
        let mut history = History::new(7);
        record(&mut history, Opening::New(None), 0, 1, "x");
        history.clear();
        assert_eq!(history.lens(), (0, 0));
        assert_eq!(history.bytes(), 0);
        assert_eq!(history.limit, 7);
    }
}
