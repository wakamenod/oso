//! What changed of a native session's rows with an event (ADR 0165), and such changes gathered
//! over several.
//!
//! Must not depend on: threads, files or the UI; plain values.

use crate::sessions::native::shown::LineStart;

/// What changed with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowChange {
    /// A row was added at the end, at this index.
    Added(usize),
    /// The row at this index changed.
    Changed(usize),
    /// The reply at `row` grew, or ended: what it draws before `at` stays, and what follows is
    /// [`render::tail`](crate::sessions::native::render::tail) from there.
    Grew { row: usize, at: LineStart },
}

impl RowChange {
    /// The row that changed.
    pub fn row(self) -> usize {
        match self {
            Self::Added(row) | Self::Changed(row) | Self::Grew { row, .. } => row,
        }
    }

    /// Of two changes to one row, which says more: an addition, then a change, then growth.
    pub fn rank(self) -> u8 {
        match self {
            Self::Added(_) => 0,
            Self::Changed(_) => 1,
            Self::Grew { .. } => 2,
        }
    }
}

/// Changes gathered over several events, one a row, in their rows' order: what says more of
/// a row wins, and of two growths the one that draws again from further up.
#[derive(Debug, Default)]
pub struct Changes(std::collections::BTreeMap<usize, RowChange>);

impl Changes {
    pub fn add(&mut self, change: RowChange) {
        let kept = self.0.entry(change.row()).or_insert(change);
        let further = match (&*kept, change) {
            (RowChange::Grew { at: kept, .. }, RowChange::Grew { at, .. }) => at.from < kept.from,
            _ => false,
        };
        if change.rank() < kept.rank() || further {
            *kept = change;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl IntoIterator for Changes {
    type Item = RowChange;
    type IntoIter = std::collections::btree_map::IntoValues<usize, RowChange>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_values()
    }
}

#[cfg(test)]
mod tests {
    use super::{Changes, RowChange};
    use crate::sessions::native::shown::LineStart;

    #[test]
    fn of_two_growths_the_one_from_further_up_is_kept() {
        let at = |from| {
            let mut at = LineStart::default();
            at.from = from;
            at
        };
        let grew = |from| RowChange::Grew {
            row: 0,
            at: at(from),
        };
        for (first, then) in [(4, 9), (9, 4)] {
            let mut changes = Changes::default();
            changes.add(grew(first));
            changes.add(grew(then));
            assert_eq!(changes.into_iter().collect::<Vec<_>>(), [grew(4)]);
        }
        let mut changes = Changes::default();
        changes.add(RowChange::Changed(0));
        changes.add(grew(4));
        assert_eq!(
            changes.into_iter().collect::<Vec<_>>(),
            [RowChange::Changed(0)]
        );
    }
}
