//! TabMap: from buffer offsets to rows and cell columns (ADR 0035).
//!
//! Must not depend on: cursors, windows or fonts; it sees only rope chunks,
//! invalid bytes and cell widths.
//!
//! The TabMap expands tabs and gives every grapheme cluster its cells (see
//! [`super::cell`]). It is a [`SumTree`] whose items mirror the rope's chunks
//! and share their text. Each item's summary holds its line breaks and the
//! *column function* of its last line: how the column at the item's end
//! depends on the column at its start. Column functions compose, so the
//! column of any offset, even on a 10 MB line full of tabs, is found in
//! O(log n) plus a scan of one chunk.
//!
//! The rope's summary stays free of display concerns, and changing the tab
//! width rebuilds only this tree.

use std::sync::Arc;

use super::DisplayCluster;
use super::cell::{CellKind, Clusters, TabWidth, holds_bidi_control};
use super::checkpoints::{self, Checkpoint, Kept, clusters_from};
use crate::decode::InvalidBytes;
use crate::rope::{Rope, RopeEdit};
use crate::sum_tree::{Bias, Cursor, Dimension, Item, SumTree, Summary};

/// A row and a cell column after tab expansion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabPoint {
    /// The row: a buffer line, while there is no folding or wrapping.
    pub row: usize,
    /// Cells from the start of the row.
    pub column: usize,
}

impl TabPoint {
    /// A point at `row` and `column`.
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// How the column after a stretch of one row depends on the column before
/// it.
///
/// Without a tab the stretch adds `lead` cells. With tabs, the first tab
/// jumps to a tab stop, and from a tab stop the rest of the stretch adds a
/// fixed number of cells (`after_tab`), because every tab stop is a multiple
/// of the tab width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ColumnFn {
    /// Cells before the first tab.
    lead: usize,
    /// Cells from the first tab's stop to the end, if there is a tab.
    after_tab: Option<usize>,
}

impl ColumnFn {
    /// A function of `lead` cells before the first tab and `after_tab` from its stop.
    pub(super) fn new(lead: usize, after_tab: Option<usize>) -> Self {
        Self { lead, after_tab }
    }

    /// Its cells before the first tab, and from its stop.
    pub(super) fn parts(self) -> (usize, Option<usize>) {
        (self.lead, self.after_tab)
    }

    pub(super) fn apply(self, column: usize, tab_width: TabWidth) -> usize {
        let column = column.saturating_add(self.lead);
        match self.after_tab {
            None => column,
            Some(cells) => tab_width.next_stop(column).saturating_add(cells),
        }
    }

    /// The function of this stretch followed by `next`.
    fn then(self, next: ColumnFn, tab_width: TabWidth) -> ColumnFn {
        match self.after_tab {
            None => ColumnFn {
                lead: self.lead.saturating_add(next.lead),
                after_tab: next.after_tab,
            },
            Some(cells) => ColumnFn {
                lead: self.lead,
                after_tab: Some(next.apply(cells, tab_width)),
            },
        }
    }

    pub(super) fn then_cells(self, cells: usize) -> ColumnFn {
        // Without a tab in `next`, `then` does not use the tab width.
        self.then(
            ColumnFn {
                lead: cells,
                after_tab: None,
            },
            TabWidth::default(),
        )
    }

    pub(super) fn then_tab(self, tab_width: TabWidth) -> ColumnFn {
        self.then(
            ColumnFn {
                lead: 0,
                after_tab: Some(0),
            },
            tab_width,
        )
    }
}

/// The summary of a stretch of text on the cell grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabSummary {
    /// Length in bytes.
    pub bytes: usize,
    /// Line breaks.
    pub lines: usize,
    /// It holds a bidirectional control, for the modeline's warning (ADR 0063).
    pub(crate) bidi_controls: bool,
    /// The column function after the last line break.
    last_line: ColumnFn,
    /// The tab width the summary was computed with. `None` only for the
    /// empty summary, which needs none.
    tab_width: Option<TabWidth>,
}

impl TabSummary {
    /// Summarizes `text`, which starts and ends on cluster boundaries.
    ///
    /// Only the text after the last line break is measured: `\r\n` holds
    /// exactly one `\n`, and a cluster always starts after `\n`. `text`
    /// starts at `base` in the buffer.
    fn from_text(text: &str, base: usize, invalid: &InvalidBytes, tab_width: TabWidth) -> Self {
        let lines = text.bytes().filter(|&byte| byte == b'\n').count();
        let tail_start = text.rfind('\n').map_or(0, |index| index + 1);
        let tail = &text[tail_start..];
        let mut last_line = ColumnFn::default();
        for (_, _, kind, cells) in Clusters::with_invalid(tail, base + tail_start, invalid) {
            last_line = match kind {
                CellKind::Tab => last_line.then_tab(tab_width),
                _ => last_line.then_cells(cells),
            };
        }
        Self {
            bytes: text.len(),
            lines,
            bidi_controls: holds_bidi_control(text),
            last_line,
            tab_width: Some(tab_width),
        }
    }
}

impl Summary for TabSummary {
    fn add_summary(&mut self, other: &Self) {
        let tab_width = self.tab_width.or(other.tab_width).unwrap_or_default();
        self.bytes += other.bytes;
        self.bidi_controls |= other.bidi_controls;
        if other.lines > 0 {
            self.lines += other.lines;
            self.last_line = other.last_line;
        } else {
            self.last_line = self.last_line.then(other.last_line, tab_width);
        }
        self.tab_width = Some(tab_width);
    }
}

impl Dimension<TabSummary> for usize {
    fn add_summary(&mut self, summary: &TabSummary) {
        *self += summary.bytes;
    }
}

impl Dimension<TabSummary> for TabPoint {
    fn add_summary(&mut self, summary: &TabSummary) {
        let tab_width = summary.tab_width.unwrap_or_default();
        if summary.lines > 0 {
            self.row += summary.lines;
            self.column = summary.last_line.apply(0, tab_width);
        } else {
            self.column = summary.last_line.apply(self.column, tab_width);
        }
    }
}

/// One item: a rope chunk's text, its summary, and the checkpoints of its long lines
/// (ADR 0099), made once scans have read the chunk often.
#[derive(Clone, Debug)]
struct TabChunk {
    text: Arc<str>,
    summary: TabSummary,
    checkpoints: Kept,
}

impl TabChunk {
    /// The last checkpoint at or before `offset` in the chunk, which starts at `base`.
    fn checkpoint_before(&self, base: usize, map: &TabMap, offset: usize) -> Option<&Checkpoint> {
        checkpoints::before_offset(self.checkpoints(base, map)?, offset)
    }

    /// Its checkpoints, if scans have read it often enough. It starts at `base`.
    fn checkpoints(&self, base: usize, map: &TabMap) -> Option<&[Checkpoint]> {
        self.checkpoints
            .get(&self.text, base, &map.invalid, map.tab_width)
    }
}

impl Item for TabChunk {
    type Summary = TabSummary;

    fn summary(&self) -> TabSummary {
        self.summary
    }
}

/// The TabMap of one buffer. Cloning is O(1) and gives a snapshot.
#[derive(Clone)]
pub struct TabMap {
    tree: SumTree<TabChunk>,
    tab_width: TabWidth,
    /// The buffer's invalid bytes, each shown as `\xNN`.
    invalid: InvalidBytes,
}

impl TabMap {
    /// Builds the map of `buffer`. O(n): it measures every cluster.
    pub fn new(buffer: &Rope, tab_width: TabWidth) -> Self {
        Self::with_invalid(buffer, &InvalidBytes::new(), tab_width)
    }

    /// [`TabMap::new`] for a buffer whose invalid bytes are `invalid`.
    pub(crate) fn with_invalid(buffer: &Rope, invalid: &InvalidBytes, tab_width: TabWidth) -> Self {
        Self {
            tree: Self::items(buffer, invalid, 0..buffer.len(), tab_width).collect(),
            tab_width,
            invalid: invalid.clone(),
        }
    }

    fn items<'a>(
        buffer: &'a Rope,
        invalid: &'a InvalidBytes,
        range: std::ops::Range<usize>,
        tab_width: TabWidth,
    ) -> impl Iterator<Item = TabChunk> + 'a {
        let mut base = range.start;
        buffer.chunk_texts(range).map(move |text| {
            let summary = TabSummary::from_text(text, base, invalid, tab_width);
            base += text.len();
            TabChunk {
                text: Arc::clone(text),
                summary,
                checkpoints: Kept::default(),
            }
        })
    }

    /// Follows one edit of the buffer. `buffer` is the rope right after the
    /// edit. Costs O(log n) plus the size of the edited chunks.
    pub fn sync(&mut self, buffer: &Rope, edit: &RopeEdit) {
        if edit.old.is_empty() && edit.new.is_empty() {
            return;
        }
        let items = Self::items(buffer, &self.invalid, edit.new.clone(), self.tab_width).collect();
        self.tree.replace_items(edit.old.clone(), items);
    }

    /// The buffer's invalid bytes.
    pub(crate) fn invalid(&self) -> &InvalidBytes {
        &self.invalid
    }

    /// Adds invalid bytes at `base` plus each entry's offset, before the
    /// [`TabMap::sync`] of the append that brought them.
    pub(crate) fn extend_invalid(&mut self, base: usize, entries: &[(usize, u8)]) {
        self.invalid.extend(base, entries);
    }

    /// The tab width.
    pub fn tab_width(&self) -> TabWidth {
        self.tab_width
    }

    /// The summary of the whole buffer.
    pub fn summary(&self) -> &TabSummary {
        self.tree.summary()
    }

    /// The point at the end of the buffer.
    pub fn max_point(&self) -> TabPoint {
        self.tree.extent()
    }

    /// The point of a buffer offset. An offset inside a cluster (or past the
    /// end) gives the point where the cluster (or the buffer) starts (ends).
    pub fn to_tab_point(&self, offset: usize) -> TabPoint {
        let mut cursor = self.tree.cursor::<(usize, TabPoint)>();
        cursor.seek(&offset, Bias::Right);
        let (start, mut point) = *cursor.start();
        let Some(item) = cursor.item() else {
            return point;
        };
        // Skip to the line of `offset` within the chunk, and on to its last checkpoint
        // before `offset` (ADR 0099).
        let bytes = &item.text.as_bytes()[..offset - start];
        let mut from = 0;
        let mut past_limit_until = None;
        let line = bytes
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map(|newline| newline + 1);
        match item.checkpoint_before(start, self, offset - start) {
            Some(checkpoint) if line.is_none_or(|line| usize::from(checkpoint.at) >= line) => {
                let rows = usize::from(checkpoint.rows);
                if rows > 0 {
                    point.row += rows;
                    point.column = checkpoint.column().apply(0, self.tab_width);
                } else {
                    point.column = checkpoint.column().apply(point.column, self.tab_width);
                }
                from = usize::from(checkpoint.at);
                past_limit_until = Some(usize::from(checkpoint.past_limit_until));
            }
            _ => {
                if let Some(line) = line {
                    point.row += bytes.iter().filter(|&&byte| byte == b'\n').count();
                    point.column = 0;
                    from = line;
                }
            }
        }
        let (clusters, shift) =
            clusters_from(&item.text, start, from, past_limit_until, &self.invalid);
        for (index, cluster, kind, cells) in clusters {
            if start + shift + index + cluster.len() > offset {
                break;
            }
            self.advance(&mut point, kind, cells);
        }
        point
    }

    /// The display cluster around `offset`, as where it starts and where it ends: `offset`
    /// twice where a cluster starts, and the end of the buffer twice past it. One seek and
    /// a scan from the nearest place before `offset` where a cluster must start, with no
    /// columns to count and no second seek to come back from them, as a display point
    /// and back would take (ADR 0091).
    pub(crate) fn cluster_at(&self, offset: usize) -> (usize, usize) {
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&offset, Bias::Right);
        let start = *cursor.start();
        let Some(item) = cursor.item() else {
            return (start, start);
        };
        // The scan starts at the last checkpoint before `offset` (ADR 0099), or after it at
        // the nearest place where two ASCII characters but CR LF meet, which always have a
        // cluster boundary between them, past which the clusters are as from the chunk's
        // start.
        let checkpoint = item.checkpoint_before(start, self, offset - start);
        let (mut from, mut past_limit_until) = checkpoint.map_or((0, None), |checkpoint| {
            (
                usize::from(checkpoint.at),
                Some(usize::from(checkpoint.past_limit_until)),
            )
        });
        let before = &item.text.as_bytes()[from..offset - start];
        if let Some(index) = before.windows(2).rposition(|pair| {
            pair[0] == b'\n' || (pair[0].is_ascii() && pair[1].is_ascii() && pair != b"\r\n")
        }) {
            from += index + 1;
            past_limit_until = None;
        }
        let (clusters, shift) =
            clusters_from(&item.text, start, from, past_limit_until, &self.invalid);
        for (index, cluster, _, _) in clusters {
            let at = start + shift + index;
            if at >= offset {
                break;
            }
            if at + cluster.len() > offset {
                return (at, at + cluster.len());
            }
        }
        (offset, offset)
    }

    /// The buffer offset of a point. A column past the end of its row gives
    /// the end of the row, before the line break; a row past the last gives
    /// the end of the buffer. A column inside a cluster that takes several
    /// cells (a wide character, a tab, an escape) gives the cluster's start
    /// with `Bias::Left` and its end with `Bias::Right`.
    pub fn to_offset(&self, target: TabPoint, bias: Bias) -> usize {
        self.locate(target, bias).0
    }

    /// [`TabMap::to_offset`], the point of the offset it returns, and the buffer
    /// offset before which characters are past a display cluster's limits, as
    /// they were when the scan reached that offset.
    fn locate(&self, target: TabPoint, bias: Bias) -> (usize, TabPoint, usize) {
        let mut cursor = self.tree.cursor::<(TabPoint, usize)>();
        cursor.seek(&target, Bias::Right);
        let (mut point, start) = *cursor.start();
        let Some(item) = cursor.item() else {
            return (start, point, 0);
        };
        // Skip to the start of the target row within the chunk, and on to its last
        // checkpoint at or before the target column (ADR 0099).
        let text = &*item.text;
        let mut from = 0;
        let rows = target.row.saturating_sub(point.row);
        if target.row > point.row
            && let Some((newline, _)) = text.match_indices('\n').nth(rows - 1)
        {
            from = newline + 1;
            point = TabPoint::new(target.row, 0);
        }
        let mut past_limit_until = None;
        if let Some((checkpoint, column)) =
            self.checkpoint_on_row((item, start), rows, point.column, target.column)
        {
            from = usize::from(checkpoint.at);
            point.column = column;
            past_limit_until = Some(usize::from(checkpoint.past_limit_until));
        }
        let (mut clusters, shift) =
            clusters_from(text, start, from, past_limit_until, &self.invalid);
        while let Some((index, cluster, kind, cells)) = clusters.next() {
            let offset = start + shift + index;
            let past_limit_until =
                |clusters: &Clusters<'_>| start + shift + clusters.past_limit_until_at(index);
            if point.row == target.row {
                if kind == CellKind::LineBreak || point.column >= target.column {
                    return (offset, point, past_limit_until(&clusters));
                }
                let mut next = point;
                self.advance(&mut next, kind, cells);
                if next.column > target.column {
                    return match bias {
                        Bias::Left => (offset, point, past_limit_until(&clusters)),
                        Bias::Right => (offset + cluster.len(), next, 0),
                    };
                }
            }
            self.advance(&mut point, kind, cells);
        }
        (start + text.len(), point, 0)
    }

    /// The last checkpoint of `item`, which starts at `base`, on its row `rows` breaks in
    /// whose column is at most `target`, with that column, `column` being where the row's part
    /// in the chunk starts.
    fn checkpoint_on_row<'a>(
        &self,
        (item, base): (&'a TabChunk, usize),
        rows: usize,
        column: usize,
        target: usize,
    ) -> Option<(&'a Checkpoint, usize)> {
        let checkpoints = item.checkpoints(base, self)?;
        checkpoints::on_row(checkpoints, (rows, column), target, self.tab_width)
    }

    /// Moves `point` past one cluster.
    fn advance(&self, point: &mut TabPoint, kind: CellKind, cells: usize) {
        match kind {
            CellKind::LineBreak => {
                point.row += 1;
                point.column = 0;
            }
            CellKind::Tab => point.column = self.tab_width.next_stop(point.column),
            CellKind::Text
            | CellKind::Control(_)
            | CellKind::Invisible(_)
            | CellKind::PastLimit(_)
            | CellKind::InvalidByte(_) => {
                point.column = point.column.saturating_add(cells);
            }
        }
    }

    /// The clusters of `start.row`, from the one at `start.column` to the
    /// end of the row.
    pub fn clusters(&self, start: TabPoint) -> RowClusters<'_> {
        let (offset, point, past_limit_until) = self.locate(start, Bias::Left);
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&offset, Bias::Right);
        let base = *cursor.start();
        let clusters = match cursor.item() {
            // Chunks end on grapheme cluster boundaries, and a grapheme cluster
            // longer than a chunk is measured piece by piece, so the chunk is
            // all the context a cluster needs. The scan says if `offset` is past
            // a limit.
            Some(item) => Clusters::with_invalid_from(
                &item.text,
                base,
                offset - base,
                past_limit_until.saturating_sub(base),
                &self.invalid,
            ),
            None => Clusters::new(""),
        };
        RowClusters {
            cursor,
            clusters,
            base,
            column: point.column,
            tab_width: self.tab_width,
            invalid: &self.invalid,
            done: false,
        }
    }
}

/// Iterates over the clusters of one row. See [`TabMap::clusters`].
pub struct RowClusters<'a> {
    cursor: Cursor<'a, TabChunk, usize>,
    clusters: Clusters<'a>,
    /// The buffer offset where `clusters` starts.
    base: usize,
    column: usize,
    tab_width: TabWidth,
    invalid: &'a InvalidBytes,
    done: bool,
}

impl<'a> Iterator for RowClusters<'a> {
    type Item = DisplayCluster<'a>;

    fn next(&mut self) -> Option<DisplayCluster<'a>> {
        if self.done {
            return None;
        }
        loop {
            if let Some((index, text, kind, cells)) = self.clusters.next() {
                let cells = match kind {
                    CellKind::LineBreak => {
                        self.done = true;
                        return None;
                    }
                    CellKind::Tab => self.tab_width.next_stop(self.column) - self.column,
                    CellKind::Text
                    | CellKind::Control(_)
                    | CellKind::Invisible(_)
                    | CellKind::PastLimit(_)
                    | CellKind::InvalidByte(_) => cells,
                };
                let cluster = DisplayCluster {
                    offset: self.base + index,
                    column: self.column,
                    cells,
                    text,
                    kind,
                };
                self.column = self.column.saturating_add(cells);
                return Some(cluster);
            }
            self.cursor.next();
            let Some(item) = self.cursor.item() else {
                self.done = true;
                return None;
            };
            self.base = *self.cursor.start();
            self.clusters = Clusters::with_invalid(&item.text, self.base, self.invalid);
        }
    }
}
