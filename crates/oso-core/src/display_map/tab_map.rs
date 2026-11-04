//! TabMap: from buffer offsets to rows and cell columns (ADR 0035).
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
use super::cell::{CellKind, Clusters, TabWidth};
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
struct ColumnFn {
    /// Cells before the first tab.
    lead: usize,
    /// Cells from the first tab's stop to the end, if there is a tab.
    after_tab: Option<usize>,
}

impl ColumnFn {
    fn apply(self, column: usize, tab_width: TabWidth) -> usize {
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

    fn then_cells(self, cells: usize) -> ColumnFn {
        // Without a tab in `next`, `then` does not use the tab width.
        self.then(
            ColumnFn {
                lead: cells,
                after_tab: None,
            },
            TabWidth::default(),
        )
    }

    fn then_tab(self, tab_width: TabWidth) -> ColumnFn {
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
            last_line,
            tab_width: Some(tab_width),
        }
    }
}

impl Summary for TabSummary {
    fn add_summary(&mut self, other: &Self) {
        let tab_width = self.tab_width.or(other.tab_width).unwrap_or_default();
        self.bytes += other.bytes;
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

/// One item: a rope chunk's text and its summary.
#[derive(Clone, Debug)]
struct TabChunk {
    text: Arc<str>,
    summary: TabSummary,
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
    /// Builds the map of `buffer`, whose invalid bytes are `invalid`. O(n):
    /// it measures every cluster.
    pub fn new(buffer: &Rope, invalid: &InvalidBytes, tab_width: TabWidth) -> Self {
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
                summary,
                text: Arc::clone(text),
            }
        })
    }

    /// Follows one edit of the buffer. `buffer` is the rope right after the
    /// edit, and `invalid` its invalid bytes. Costs O(log n) plus the size
    /// of the edited chunks.
    pub fn sync(&mut self, buffer: &Rope, invalid: &InvalidBytes, edit: &RopeEdit) {
        self.invalid = invalid.clone();
        if edit.old.is_empty() && edit.new.is_empty() {
            return;
        }
        let (mut tree, _) = self.tree.split(&edit.old.start, Bias::Right);
        let (_, right) = self.tree.split(&edit.old.end, Bias::Right);
        tree.append(Self::items(buffer, invalid, edit.new.clone(), self.tab_width).collect());
        tree.append(right);
        self.tree = tree;
    }

    /// The buffer's invalid bytes.
    pub fn invalid(&self) -> &InvalidBytes {
        &self.invalid
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
        // Skip to the line of `offset` within the chunk.
        let bytes = &item.text.as_bytes()[..offset - start];
        let mut from = 0;
        if let Some(newline) = bytes.iter().rposition(|&byte| byte == b'\n') {
            point.row += bytes.iter().filter(|&&byte| byte == b'\n').count();
            point.column = 0;
            from = newline + 1;
        }
        let clusters = Clusters::with_invalid(&item.text[from..], start + from, &self.invalid);
        for (index, cluster, kind, cells) in clusters {
            if start + from + index + cluster.len() > offset {
                break;
            }
            self.advance(&mut point, kind, cells);
        }
        point
    }

    /// The buffer offset of a point. A column past the end of its row gives
    /// the end of the row, before the line break; a row past the last gives
    /// the end of the buffer. A column inside a cluster that takes several
    /// cells (a wide character, a tab, an escape) gives the cluster's start
    /// with `Bias::Left` and its end with `Bias::Right`.
    pub fn to_offset(&self, target: TabPoint, bias: Bias) -> usize {
        self.locate(target, bias).0
    }

    /// [`TabMap::to_offset`], and the point of the offset it returns.
    fn locate(&self, target: TabPoint, bias: Bias) -> (usize, TabPoint) {
        let mut cursor = self.tree.cursor::<(TabPoint, usize)>();
        cursor.seek(&target, Bias::Right);
        let (mut point, start) = *cursor.start();
        let Some(item) = cursor.item() else {
            return (start, point);
        };
        // Skip to the start of the target row within the chunk.
        let text = &*item.text;
        let mut from = 0;
        if target.row > point.row
            && let Some((newline, _)) = text.match_indices('\n').nth(target.row - point.row - 1)
        {
            from = newline + 1;
            point = TabPoint::new(target.row, 0);
        }
        for (index, cluster, kind, cells) in
            Clusters::with_invalid(&text[from..], start + from, &self.invalid)
        {
            let offset = start + from + index;
            if point.row == target.row {
                if kind == CellKind::LineBreak || point.column >= target.column {
                    return (offset, point);
                }
                let mut next = point;
                self.advance(&mut next, kind, cells);
                if next.column > target.column {
                    return match bias {
                        Bias::Left => (offset, point),
                        Bias::Right => (offset + cluster.len(), next),
                    };
                }
            }
            self.advance(&mut point, kind, cells);
        }
        (start + text.len(), point)
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
            | CellKind::InvalidByte(_) => {
                point.column = point.column.saturating_add(cells);
            }
        }
    }

    /// The clusters of `start.row`, from the one at `start.column` to the
    /// end of the row.
    pub fn clusters(&self, start: TabPoint) -> RowClusters<'_> {
        let (offset, point) = self.locate(start, Bias::Left);
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&offset, Bias::Right);
        let clusters = match cursor.item() {
            Some(item) => Clusters::with_invalid(
                &item.text[offset - *cursor.start()..],
                offset,
                &self.invalid,
            ),
            None => Clusters::new(""),
        };
        RowClusters {
            cursor,
            clusters,
            base: offset,
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
