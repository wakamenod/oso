//! Selection sets: a window's cursors (ADR 0007, ADR 0039).
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
pub struct Update<E> {
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

    /// The primary selection.
    pub fn primary(&self) -> &Selection {
        &self.selections[self.primary]
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
    pub fn update<E>(&mut self, mut f: impl FnMut(&mut Selection) -> Result<(), E>) -> Update<E> {
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
        Update { changed, error }
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
            Update {
                changed: true,
                error: Some("End of buffer")
            }
        );
        assert_eq!(set.primary().cursor, 4);
        let update = set.update(|_| Ok::<(), ()>(()));
        assert!(!update.changed);
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
