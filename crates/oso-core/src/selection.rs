//! Selection sets: a window's cursors (ADR 0007, ADR 0040).
//!
//! Must not depend on: `buffer`, `motion` or `window`; it holds offsets and a
//! goal column, and `motion` decides how they move.
//!
//! A window has one or more selections. Commands take the whole set and
//! move every selection, so multiple cursors (M2) need no new command
//! signatures. Each selection's mark arrives with marks and regions (M2).

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
}
