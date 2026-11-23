//! What changed of a native session's rows with an event (ADR 0165), and such changes gathered
//! over several.
//!
//! Must not depend on: threads, files or the UI; plain values.

/// What changed with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowChange {
    /// A row was added at the end, at this index.
    Added(usize),
    /// The row at this index changed.
    Changed(usize),
    /// The reply at `row` grew: its first `lines` lines as drawn stay, and what follows them
    /// is [`render::tail`](crate::sessions::native::render::tail) from the text's byte `from`.
    Grew {
        row: usize,
        lines: usize,
        from: usize,
    },
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
/// a row wins, and of two growths the first, which the later ones grew on.
#[derive(Debug, Default)]
pub struct Changes(std::collections::BTreeMap<usize, RowChange>);

impl Changes {
    pub fn add(&mut self, change: RowChange) {
        let kept = self.0.entry(change.row()).or_insert(change);
        if change.rank() < kept.rank() {
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
