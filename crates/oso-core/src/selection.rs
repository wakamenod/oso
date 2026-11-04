//! Selection sets: a window's cursors (ADR 0007, ADR 0040).
//!
//! Must not depend on: `buffer`, `motion` or `window`; it holds offsets and a
//! goal column, and `motion` decides how they move.
//!
//! A window has one or more selections. Commands take the whole set and
//! move every selection, so multiple cursors (M2) need no new command
//! signatures. Each selection's mark arrives with marks and regions (M2).
//!
//! Cursors follow edits here, as offsets ([`SelectionSet::follow_edits`], ADR
//! 0052). The editor then clips them to the display clusters of the new text.

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

/// One selection: a cursor, and the goal of a vertical move in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Selection {
    /// Point: a buffer offset at the start of a display cluster.
    pub cursor: usize,
    /// Set by vertical motion and kept by the next one; every other motion
    /// clears it.
    pub goal: Option<Goal>,
}

impl Selection {
    /// A selection with its cursor at `offset` and no goal.
    pub const fn at(offset: usize) -> Self {
        Self {
            cursor: offset,
            goal: None,
        }
    }
}

/// A window's selections: never empty, sorted by cursor, with no two
/// cursors at one offset. One of them is primary: the one the window
/// scrolls to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionSet {
    selections: Vec<Selection>,
    primary: usize,
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

/// What [`SelectionSet::update`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct UpdateOutcome<E> {
    /// Some cursor moved. A changed goal alone shows nothing.
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
        }
    }

    /// The set of `selections`, with the one at position `primary` primary,
    /// sorted and with those at one offset merged. `None` if there is no such
    /// position, which also rules out an empty set.
    pub fn from_parts(selections: Vec<Selection>, primary: usize) -> Option<Self> {
        (primary < selections.len()).then(|| {
            let mut set = Self {
                selections,
                primary,
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

    /// The number of selections; at least 1.
    pub fn len(&self) -> usize {
        self.selections.len()
    }

    /// Always false: a set has at least one selection.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Runs `f` on every selection, then merges selections whose cursors
    /// met. A selection that fails still keeps what `f` changed, as an
    /// Emacs motion that hits the buffer end moves as far as it can.
    pub fn update<E>(
        &mut self,
        mut f: impl FnMut(&mut Selection) -> Result<(), E>,
    ) -> UpdateOutcome<E> {
        let mut changed = false;
        let mut error = None;
        for selection in &mut self.selections {
            let before = selection.cursor;
            if let Err(e) = f(selection) {
                error.get_or_insert(e);
            }
            changed |= selection.cursor != before;
        }
        if changed {
            self.normalize();
        }
        UpdateOutcome { changed, error }
    }

    /// Moves every cursor through `edits`, which are sorted, do not overlap
    /// and are all in the offsets before them. An edit is not a vertical
    /// move, so goals are cleared. O(cursors + edits).
    ///
    /// A cursor before an edit keeps its offset, and one after it shifts. The
    /// `bias` decides the rest:
    ///
    /// - `Bias::Right`, for the window that edits: a cursor at the start of an
    ///   edit, or inside its replaced text, ends after the new text.
    /// - `Bias::Left`, for other windows, as an Emacs marker: a cursor at the
    ///   start of an edit stays before the new text, and one inside the
    ///   replaced text moves there too.
    ///
    /// With either, a cursor at the end of the replaced text stays before the
    /// text that followed it.
    pub(crate) fn follow_edits(&mut self, edits: &[OffsetEdit], bias: Bias) {
        // Where the edits passed so far end, before and after them. Cursors
        // are sorted, so no later cursor comes before `old_end`.
        let (mut old_end, mut new_end, mut next) = (0usize, 0usize, 0);
        self.update(|selection| {
            let cursor = selection.cursor;
            let moved = loop {
                let Some(edit) = edits.get(next) else {
                    break new_end.saturating_add(cursor - old_end);
                };
                let new_start = new_end + (edit.start - old_end);
                let before = match bias {
                    Bias::Left => cursor <= edit.start,
                    Bias::Right => cursor < edit.start,
                };
                if before {
                    break new_end.saturating_add(cursor - old_end);
                }
                if cursor < edit.start + edit.old_len {
                    break match bias {
                        Bias::Left => new_start,
                        Bias::Right => new_start + edit.new_len,
                    };
                }
                old_end = edit.start + edit.old_len;
                new_end = new_start + edit.new_len;
                next += 1;
            };
            selection.cursor = moved;
            selection.goal = None;
            Ok::<(), ()>(())
        });
    }

    /// Sorts the selections and merges those at one offset, keeping the
    /// primary one.
    fn normalize(&mut self) {
        let primary = self.selections[self.primary].cursor;
        self.selections.sort_by_key(|selection| selection.cursor);
        self.selections.dedup_by_key(|selection| selection.cursor);
        self.primary = self
            .selections
            .partition_point(|selection| selection.cursor < primary);
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
    fn no_edits_only_clear_goals() {
        assert_eq!(followed(&[3, 7], &[], Bias::Right), [3, 7]);
    }
}
