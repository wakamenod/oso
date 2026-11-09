//! Reading the TabMap by offset rather than by point (ADR 0101): display clusters forward
//! from an offset and backward from one, with their kind and text and no columns, which
//! word motion and word checks read; and the column an offset has once an earlier one on
//! its line moved, from the summaries between, which `TAB` at several cursors needs.
//!
//! Must not depend on: cursors, windows or fonts.
//!
//! A scan starts where the clusters are as a scan from the chunk's start finds them: two
//! ASCII characters but CR LF, a checkpoint (ADR 0099), or the chunk's start. `cluster_at`
//! starts there too. Going back, it reads a stride at a time and gives the clusters of each
//! stretch from the last.

use super::{CellKind, Clusters, ColumnFn, TabChunk, TabMap, checkpoints, clusters_from};
use crate::sum_tree::{Bias, Cursor};

/// A display cluster as word motion reads it: where it starts, its text and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece<'a> {
    pub offset: usize,
    pub text: &'a str,
    pub kind: CellKind,
}

/// Whether two ASCII characters, but CR LF, or a line break and what follows meet in
/// `pair`, so that a cluster starts between them.
fn breaks(pair: &[u8]) -> bool {
    pair[0] == b'\n' || (pair[0].is_ascii() && pair[1].is_ascii() && pair != b"\r\n")
}

impl TabMap {
    /// Where a scan of `item`, which starts at `start`, may begin to find the cluster at
    /// `offset` in it, and the past-limit state there if it is a checkpoint: the nearest
    /// place within a stride where two ASCII characters meet, found from the bytes alone,
    /// else the last checkpoint before `offset` or the nearest such place after it, else the
    /// chunk's start.
    pub(super) fn scan_start(
        &self,
        item: &TabChunk,
        start: usize,
        offset: usize,
    ) -> (usize, Option<usize>) {
        let bytes = item.text.as_bytes();
        let near = offset.saturating_sub(checkpoints::STRIDE);
        let pair = |range: std::ops::Range<usize>| bytes[range].windows(2).rposition(breaks);
        if let Some(index) = pair(near..offset) {
            return (near + index + 1, None);
        }
        let (at, past) = self.checkpoint_scan(item, start, offset);
        // A checkpoint within the stride already searched needs no search after it.
        match pair(at.min(near)..near) {
            Some(index) => (at.min(near) + index + 1, None),
            None => (at, past),
        }
    }

    /// Where a scan of `item`, which starts at `start`, begins from the last checkpoint before
    /// `offset` in it, and the past-limit state there; the chunk's start with none.
    fn checkpoint_scan(
        &self,
        item: &TabChunk,
        start: usize,
        offset: usize,
    ) -> (usize, Option<usize>) {
        item.checkpoint_before(start, self, offset)
            .map_or((0, None), |checkpoint| {
                let past = usize::from(checkpoint.past_limit_until);
                (usize::from(checkpoint.at), Some(past))
            })
    }

    /// The column at `to` once `from`, a cluster's start on the same line before it, is at
    /// `column`, as after an insertion before `from`: what lies between keeps its cells, and
    /// a tab reaches the next stop from where it now starts. In O(log n) and a scan of the
    /// chunks the two are in, from the summaries of the chunks between (ADR 0101).
    pub(crate) fn column_after(&self, from: usize, column: usize, to: usize) -> usize {
        if to <= from {
            return column;
        }
        debug_assert_eq!(
            self.to_tab_point(from).row,
            self.to_tab_point(to).row,
            "`from` and `to` are on one line"
        );
        let tab_width = self.tab_width;
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&from, Bias::Right);
        let first = *cursor.start();
        let Some(item) = cursor.item() else {
            return column;
        };
        let first_end = first + item.text.len();
        let head = self.column_fn(item, first, from - first, to.min(first_end) - first);
        if to <= first_end {
            return head.apply(column, tab_width);
        }
        cursor.seek(&(to - 1), Bias::Right);
        let last = *cursor.start();
        let Some(tail_item) = cursor.item() else {
            return head.apply(column, tab_width);
        };
        // The chunks wholly between, from their summaries, copying nothing: no line breaks
        // among them, as `from` and `to` are on one line.
        let middle = self.tree.summary_in(&(first_end..last)).last_line;
        let tail = self.column_fn(tail_item, last, 0, to - last);
        tail.apply(
            middle.apply(head.apply(column, tab_width), tab_width),
            tab_width,
        )
    }

    /// The column function of `item`, which starts at `start`, from `from`, a cluster's
    /// start, to `to` in it, on one line.
    fn column_fn(&self, item: &TabChunk, start: usize, from: usize, to: usize) -> ColumnFn {
        let (scan, past) = self.scan_start(item, start, from);
        let (clusters, shift) = clusters_from(&item.text, start, scan, past, &self.invalid);
        let mut function = ColumnFn::default();
        for (index, _, kind, cells) in clusters {
            let at = shift + index;
            if at >= to {
                break;
            }
            if at < from {
                continue;
            }
            function = match kind {
                CellKind::Tab => function.then_tab(self.tab_width),
                _ => function.then_cells(cells),
            };
        }
        function
    }

    /// The display clusters from the one at `offset` on to the end of the buffer, line
    /// breaks among them.
    pub(crate) fn pieces_after(&self, offset: usize) -> PiecesAfter<'_> {
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&offset, Bias::Right);
        let base = *cursor.start();
        let clusters = match cursor.item() {
            Some(item) => {
                let (from, past) = self.scan_start(item, base, offset - base);
                Some(clusters_from(&item.text, base, from, past, &self.invalid))
            }
            None => None,
        };
        PiecesAfter {
            map: self,
            cursor,
            clusters,
            base,
            from: offset,
        }
    }

    /// The display clusters that start before `offset`, from the nearest back to the start
    /// of the buffer, line breaks among them. The first is the one `offset` is in, if it is
    /// in one.
    pub(crate) fn pieces_before(&self, offset: usize) -> PiecesBefore<'_> {
        PiecesBefore {
            map: self,
            end: offset,
            stretch: Vec::new(),
            refills: 0,
        }
    }
}

/// The clusters from an offset on. See [`TabMap::pieces_after`].
pub struct PiecesAfter<'a> {
    map: &'a TabMap,
    cursor: Cursor<'a, TabChunk, usize>,
    /// The clusters of the chunk under the cursor, and where their offsets count from in it.
    clusters: Option<(Clusters<'a>, usize)>,
    /// Where the chunk under the cursor starts.
    base: usize,
    /// Clusters before this offset are skipped, as the scan began before it.
    from: usize,
}

impl<'a> Iterator for PiecesAfter<'a> {
    type Item = Piece<'a>;

    fn next(&mut self) -> Option<Piece<'a>> {
        loop {
            let (clusters, shift) = self.clusters.as_mut()?;
            if let Some((index, text, kind, _)) = clusters.next() {
                let offset = self.base + *shift + index;
                if offset + text.len() <= self.from {
                    continue;
                }
                return Some(Piece { offset, text, kind });
            }
            self.cursor.next();
            let item = self.cursor.item()?;
            self.base = *self.cursor.start();
            self.clusters = Some((
                Clusters::with_invalid(&item.text, self.base, &self.map.invalid),
                0,
            ));
        }
    }
}

/// The clusters before an offset, from the nearest back. See [`TabMap::pieces_before`].
pub struct PiecesBefore<'a> {
    map: &'a TabMap,
    /// Where the stretch to read next ends.
    end: usize,
    /// The clusters of the stretch read last, in order, the next to give last.
    stretch: Vec<Piece<'a>>,
    refills: usize,
}

impl PiecesBefore<'_> {
    /// How many stretches it has read, each a seek and at most a stride and a cluster once
    /// its chunk has checkpoints, for a walk to charge.
    pub(crate) fn refills(&self) -> usize {
        self.refills
    }
}

impl<'a> Iterator for PiecesBefore<'a> {
    type Item = Piece<'a>;

    fn next(&mut self) -> Option<Piece<'a>> {
        if let Some(piece) = self.stretch.pop() {
            return Some(piece);
        }
        if self.end == 0 {
            return None;
        }
        let map = self.map;
        let mut cursor = map.tree.cursor::<usize>();
        cursor.seek(&(self.end - 1), Bias::Right);
        let base = *cursor.start();
        let item = cursor.item()?;
        let end = self.end - base;
        // The stretch starts at the first place within a stride before `end` where a
        // cluster surely starts, so that each stretch reads about a stride.
        let near = end.saturating_sub(checkpoints::STRIDE);
        let bytes = item.text.as_bytes();
        let (from, past) = match bytes[near..end].windows(2).position(breaks) {
            Some(index) => (near + index + 1, None),
            None => map.checkpoint_scan(item, base, end - 1),
        };
        let (clusters, shift) = clusters_from(&item.text, base, from, past, &map.invalid);
        for (index, text, kind, _) in clusters {
            let offset = base + shift + index;
            if offset >= self.end {
                break;
            }
            self.stretch.push(Piece { offset, text, kind });
        }
        self.refills += 1;
        self.end = base + from;
        self.stretch.pop()
    }
}
