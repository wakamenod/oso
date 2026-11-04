//! The display model: maps buffer offsets to display rows and cell columns
//! and back, in O(log n) (`docs/dev/display-model.md`, ADR 0006,
//! ADR 0035).
//!
//! The full pipeline is:
//!
//! ```text
//! Buffer (Rope)
//!   → FoldMap → InlayMap   (not yet: input coordinates are buffer offsets)
//!   → TabMap               tabs and cluster widths on the cell grid
//!   → WrapMap → BlockMap   (not yet: rows are buffer lines)
//! DisplayPoint
//! ```
//!
//! Only the [`TabMap`] exists so far, because soft wrap is off by default
//! and the viewer needs nothing else. Each missing layer slots in where the
//! diagram shows it, as its own [`SumTree`](crate::sum_tree::SumTree) that
//! follows the edits of the layer below:
//!
//! - FoldMap and InlayMap go below the TabMap. The TabMap's items then
//!   mirror InlayMap chunks instead of rope chunks, and its input
//!   coordinate becomes an inlay offset.
//! - WrapMap and BlockMap go above it. A [`DisplayPoint`] then differs from
//!   a [`TabPoint`]: one buffer line can span several rows.
//!
//! Callers use only [`DisplayMap`] and [`DisplaySnapshot`], so adding a
//! layer does not change their code.

pub mod cell;
pub mod tab_map;

use crate::decode::InvalidBytes;
use crate::rope::{Rope, RopeEdit};
use crate::sum_tree::Bias;

pub use cell::{CellKind, Clusters, Escape, TabWidth, escape};
pub use tab_map::{TabMap, TabPoint};

/// A position on screen: a display row and a cell column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayPoint {
    /// The display row. Today this is the buffer line.
    pub row: usize,
    /// Cells from the start of the row.
    pub column: usize,
}

impl DisplayPoint {
    /// A point at `row` and `column`.
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

impl From<TabPoint> for DisplayPoint {
    fn from(point: TabPoint) -> Self {
        Self::new(point.row, point.column)
    }
}

impl From<DisplayPoint> for TabPoint {
    fn from(point: DisplayPoint) -> Self {
        Self::new(point.row, point.column)
    }
}

/// One grapheme cluster on a display row, for rendering and hit testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayCluster<'a> {
    /// The buffer offset where the cluster starts.
    pub offset: usize,
    /// The first cell it takes.
    pub column: usize,
    /// How many cells it takes. A tab takes the cells up to its tab stop.
    pub cells: usize,
    /// The cluster's text in the buffer.
    pub text: &'a str,
    /// What to draw: the text, blank cells for a tab, or an escape (boxed
    /// for an invisible character). The renderer clips each cluster's glyphs
    /// to its cells, so stacked combining marks cannot paint over other
    /// rows.
    pub kind: CellKind,
}

/// The display model of one buffer, kept in step with its edits.
#[derive(Clone)]
pub struct DisplayMap {
    tab_map: TabMap,
}

impl DisplayMap {
    /// Builds the display model of `buffer`. O(n): it measures every
    /// cluster, so large buffers build it off the UI thread, chunk by chunk
    /// as they load (see [`DisplayMap::sync`]).
    pub fn new(buffer: &Rope, tab_width: TabWidth) -> Self {
        Self {
            tab_map: TabMap::new(buffer, tab_width),
        }
    }

    /// Follows one edit of the buffer, including an append while loading.
    /// `buffer` is the rope right after that edit; call this once per edit,
    /// in order. Costs O(log n) plus the size of the edited chunks.
    pub fn sync(&mut self, buffer: &Rope, edit: &RopeEdit) {
        self.tab_map.sync(buffer, edit);
    }

    /// Adds invalid bytes, each shown as `\xNN`, at `base` plus each
    /// entry's offset. The display map is the one owner of a buffer's
    /// invalid-byte table (ADR 0040). Call it before the [`DisplayMap::sync`]
    /// of the append that brought them.
    pub(crate) fn extend_invalid(&mut self, base: usize, entries: &[(usize, u8)]) {
        self.tab_map.extend_invalid(base, entries);
    }

    /// The buffer's invalid bytes.
    pub fn invalid_bytes(&self) -> &InvalidBytes {
        self.tab_map.invalid()
    }

    /// The tab width.
    pub fn tab_width(&self) -> TabWidth {
        self.tab_map.tab_width()
    }

    /// Changes the tab width. O(n), like [`DisplayMap::new`] (about 25 ms
    /// per 100 MB), so large buffers do it off the UI thread.
    pub fn set_tab_width(&mut self, buffer: &Rope, tab_width: TabWidth) {
        let invalid = self.tab_map.invalid().clone();
        self.tab_map = TabMap::with_invalid(buffer, &invalid, tab_width);
    }

    /// An immutable snapshot. O(1); it can be sent to another thread.
    pub fn snapshot(&self) -> DisplaySnapshot {
        DisplaySnapshot {
            tab_map: self.tab_map.clone(),
        }
    }
}

/// An immutable view of the display model. All conversions take
/// O(log n) plus a scan of one chunk, independent of line length.
#[derive(Clone)]
pub struct DisplaySnapshot {
    tab_map: TabMap,
}

impl DisplaySnapshot {
    /// The buffer's invalid bytes.
    pub fn invalid_bytes(&self) -> &InvalidBytes {
        self.tab_map.invalid()
    }

    /// The number of display rows.
    pub fn row_count(&self) -> usize {
        self.tab_map.summary().lines + 1
    }

    /// The point at the end of the buffer.
    pub fn max_point(&self) -> DisplayPoint {
        self.tab_map.max_point().into()
    }

    /// The display point of a buffer offset. An offset inside a grapheme
    /// cluster, including between the `\r` and `\n` of a line break, gives
    /// the point where the cluster starts. An offset past the end gives the
    /// end.
    pub fn offset_to_display_point(&self, offset: usize) -> DisplayPoint {
        self.tab_map.to_tab_point(offset).into()
    }

    /// The buffer offset of a display point. A column past the end of its
    /// row gives the end of the row, before the line break; a row past the
    /// last gives the end of the buffer. A column inside a cluster that
    /// takes several cells (a wide character, a tab, a control escape) gives
    /// the cluster's start with `Bias::Left` and its end with `Bias::Right`.
    pub fn display_point_to_offset(&self, point: DisplayPoint, bias: Bias) -> usize {
        self.tab_map.to_offset(point.into(), bias)
    }

    /// Moves `point` to the nearest position a cursor can take, as
    /// [`DisplaySnapshot::display_point_to_offset`] does.
    pub fn clip_point(&self, point: DisplayPoint, bias: Bias) -> DisplayPoint {
        self.offset_to_display_point(self.display_point_to_offset(point, bias))
    }

    /// The width of `row` in cells. 0 for a row past the last.
    pub fn row_cells(&self, row: usize) -> usize {
        if row >= self.row_count() {
            return 0;
        }
        let end = self.display_point_to_offset(DisplayPoint::new(row, usize::MAX), Bias::Left);
        self.offset_to_display_point(end).column
    }

    /// The clusters of `start.row`, from the one that covers `start.column`
    /// to the end of the row. A renderer takes clusters until the window is
    /// full, so the cost depends on the visible columns, not the line length.
    pub fn clusters(&self, start: DisplayPoint) -> tab_map::RowClusters<'_> {
        self.tab_map.clusters(start.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rope::tests::{floor_boundary, has_long_cluster, text_strategy};
    use proptest::prelude::*;
    use unicode_segmentation::UnicodeSegmentation;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct NaiveCluster {
        start: usize,
        end: usize,
        column: usize,
        cells: usize,
        kind: CellKind,
    }

    struct NaiveRow {
        clusters: Vec<NaiveCluster>,
        content_end: usize,
        width: usize,
    }

    /// The display rows by a linear scan of each whole line.
    fn naive_rows(text: &str, tab_width: TabWidth) -> Vec<NaiveRow> {
        let mut rows = Vec::new();
        let mut line_start = 0;
        loop {
            let newline = text[line_start..]
                .find('\n')
                .map(|index| line_start + index);
            let mut content_end = newline.unwrap_or(text.len());
            if newline.is_some() && text[..content_end].ends_with('\r') {
                content_end -= 1;
            }
            let mut column = 0;
            let mut clusters = Vec::new();
            let pieces = text[line_start..content_end]
                .grapheme_indices(true)
                .flat_map(|(index, cluster)| display_pieces(cluster, index));
            for (index, cluster) in pieces {
                let (kind, cells) = cell::classify(cluster);
                let cells = match kind {
                    CellKind::Tab => tab_width.next_stop(column) - column,
                    _ => cells,
                };
                let start = line_start + index;
                clusters.push(NaiveCluster {
                    start,
                    end: start + cluster.len(),
                    column,
                    cells,
                    kind,
                });
                column += cells;
            }
            rows.push(NaiveRow {
                clusters,
                content_end,
                width: column,
            });
            match newline {
                Some(newline) => line_start = newline + 1,
                None => return rows,
            }
        }
    }

    /// A grapheme cluster split into display clusters, with offsets.
    fn display_pieces(cluster: &str, offset: usize) -> Vec<(usize, &str)> {
        let mut pieces = Vec::new();
        let mut start = 0;
        while start < cluster.len() {
            let len = cell::first_display_cluster(&cluster[start..]);
            pieces.push((offset + start, &cluster[start..start + len]));
            start += len;
        }
        pieces
    }

    fn naive_to_point(text: &str, rows: &[NaiveRow], offset: usize) -> DisplayPoint {
        let offset = floor_boundary(text, offset);
        let row = text[..offset].matches('\n').count();
        let line = &rows[row];
        let column = line
            .clusters
            .iter()
            .find(|cluster| cluster.start <= offset && offset < cluster.end)
            .map_or(line.width, |cluster| cluster.column);
        DisplayPoint::new(row, column)
    }

    fn naive_to_offset(text: &str, rows: &[NaiveRow], point: DisplayPoint, bias: Bias) -> usize {
        let Some(line) = rows.get(point.row) else {
            return text.len();
        };
        for cluster in &line.clusters {
            if point.column < cluster.column + cluster.cells {
                if point.column == cluster.column || bias == Bias::Left {
                    return cluster.start;
                }
                return cluster.end;
            }
        }
        line.content_end
    }

    fn check_against(snapshot: &DisplaySnapshot, text: &str, tab_width: TabWidth) {
        if has_long_cluster(text) {
            // Split between chunks on purpose; see `long_clusters_are_split`.
            return;
        }
        let rows = naive_rows(text, tab_width);
        assert_eq!(snapshot.row_count(), rows.len());
        let last = rows.len() - 1;
        assert_eq!(
            snapshot.max_point(),
            DisplayPoint::new(last, rows[last].width)
        );
        for offset in 0..=text.len() + 1 {
            assert_eq!(
                snapshot.offset_to_display_point(offset),
                naive_to_point(text, &rows, offset),
                "offset {offset} in {text:?}"
            );
        }
        for row in 0..=rows.len() {
            let width = rows.get(row).map_or(0, |line| line.width);
            assert_eq!(snapshot.row_cells(row), width, "row {row}");
            for column in 0..=width + 2 {
                let point = DisplayPoint::new(row, column);
                for bias in [Bias::Left, Bias::Right] {
                    assert_eq!(
                        snapshot.display_point_to_offset(point, bias),
                        naive_to_offset(text, &rows, point, bias),
                        "{point:?} {bias:?} in {text:?}"
                    );
                }
            }
            let Some(line) = rows.get(row) else {
                continue;
            };
            // Clusters from each column match the naive row's tail.
            for column in [0, 1, width / 2, width.saturating_sub(1), width] {
                let got: Vec<NaiveCluster> = snapshot
                    .clusters(DisplayPoint::new(row, column))
                    .map(|cluster| NaiveCluster {
                        start: cluster.offset,
                        end: cluster.offset + cluster.text.len(),
                        column: cluster.column,
                        cells: cluster.cells,
                        kind: cluster.kind,
                    })
                    .collect();
                let first = line
                    .clusters
                    .iter()
                    .position(|cluster| column < cluster.column + cluster.cells)
                    .unwrap_or(line.clusters.len());
                assert_eq!(got, line.clusters[first..], "row {row} from {column}");
            }
        }
    }

    #[test]
    fn empty_buffer() {
        let rope = Rope::new();
        let snapshot = DisplayMap::new(&rope, TabWidth::default()).snapshot();
        check_against(&snapshot, "", TabWidth::default());
        assert_eq!(snapshot.clusters(DisplayPoint::new(3, 3)).count(), 0);
    }

    #[test]
    fn tabs_and_wide_characters() {
        let text = "a\tb\n日本\t語\r\n\t\t\n\r";
        let rope = Rope::from(text);
        let snapshot = DisplayMap::new(&rope, TabWidth::new(4)).snapshot();
        check_against(&snapshot, text, TabWidth::new(4));
        assert_eq!(snapshot.offset_to_display_point(2), DisplayPoint::new(0, 4));
        assert_eq!(snapshot.row_cells(1), 10);
        assert_eq!(snapshot.row_cells(3), 2, "a lone CR shows as ^M");
        let inside_tab = DisplayPoint::new(0, 2);
        assert_eq!(snapshot.display_point_to_offset(inside_tab, Bias::Left), 1);
        assert_eq!(snapshot.display_point_to_offset(inside_tab, Bias::Right), 2);
        assert_eq!(
            snapshot.clip_point(DisplayPoint::new(1, 1), Bias::Left),
            DisplayPoint::new(1, 0),
            "the middle of 日"
        );
    }

    #[test]
    fn changing_the_tab_width_rebuilds() {
        let text = "\tx\n";
        let rope = Rope::from(text);
        let mut map = DisplayMap::new(&rope, TabWidth::default());
        assert_eq!(map.snapshot().row_cells(0), 9);
        map.set_tab_width(&rope, TabWidth::new(2));
        assert_eq!(map.tab_width(), TabWidth::new(2));
        check_against(&map.snapshot(), text, TabWidth::new(2));
    }

    #[test]
    fn long_line_far_columns() {
        let text = "ab\t日本語x\u{301}\t".repeat(20_000);
        let tab_width = TabWidth::new(4);
        let rope = Rope::from(text.as_str());
        let snapshot = DisplayMap::new(&rope, tab_width).snapshot();
        let rows = naive_rows(&text, tab_width);
        assert_eq!(snapshot.row_cells(0), rows[0].width);
        for step in [1, 7, 4_999, 99_991] {
            for offset in (0..text.len()).step_by(step).take(50) {
                assert_eq!(
                    snapshot.offset_to_display_point(offset),
                    naive_to_point(&text, &rows, offset)
                );
            }
            for column in (0..rows[0].width).step_by(step).take(50) {
                let point = DisplayPoint::new(0, column);
                for bias in [Bias::Left, Bias::Right] {
                    assert_eq!(
                        snapshot.display_point_to_offset(point, bias),
                        naive_to_offset(&text, &rows, point, bias)
                    );
                }
            }
        }
        let far = DisplayPoint::new(0, rows[0].width - 10);
        let visible: Vec<_> = snapshot.clusters(far).take(3).collect();
        assert_eq!(visible.len(), 3);
        assert!(visible[0].column <= far.column);
    }

    #[test]
    fn long_clusters_are_split() {
        // A cluster longer than a chunk is measured piece by piece: wider
        // than 2 cells, but every mapping stays consistent and nothing
        // panics.
        let text = "e".to_owned() + &"\u{301}".repeat(crate::rope::MAX_CHUNK_BYTES * 2);
        let rope = Rope::from(text.as_str());
        let snapshot = DisplayMap::new(&rope, TabWidth::default()).snapshot();
        let width = snapshot.row_cells(0);
        assert!(width > 2);
        for column in 0..=width {
            let offset = snapshot.display_point_to_offset(DisplayPoint::new(0, column), Bias::Left);
            assert!(snapshot.offset_to_display_point(offset).column <= column);
        }
        let cells: usize = snapshot
            .clusters(DisplayPoint::default())
            .map(|c| c.cells)
            .sum();
        assert_eq!(cells, width);
    }

    #[test]
    fn snapshots_are_send_and_sync() {
        fn assert_send_sync<X: Send + Sync>() {}
        assert_send_sync::<DisplaySnapshot>();
        assert_send_sync::<Rope>();
    }

    proptest! {
        #[test]
        fn mappings_match_a_naive_scan(text in text_strategy(50), tab_width in 1u32..10) {
            let tab_width = TabWidth::new(tab_width);
            let snapshot = DisplayMap::new(&Rope::from(text.as_str()), tab_width).snapshot();
            check_against(&snapshot, &text, tab_width);
        }

        #[test]
        fn sync_follows_edits(
            text in text_strategy(40),
            edits in prop::collection::vec((0usize..300, 0usize..300, text_strategy(10)), 1..10),
            tab_width in 1u32..10,
        ) {
            let tab_width = TabWidth::new(tab_width);
            let mut rope = Rope::from(text.as_str());
            let mut map = DisplayMap::new(&rope, tab_width);
            let mut model = text;
            for (a, b, insert) in edits {
                let start = floor_boundary(&model, a.min(b));
                let end = floor_boundary(&model, a.max(b));
                let before = map.snapshot();
                let before_text = model.clone();
                let edit = rope.replace(start..end, &insert);
                map.sync(&rope, &edit);
                model.replace_range(start..end, &insert);
                check_against(&map.snapshot(), &model, tab_width);
                check_against(&before, &before_text, tab_width);
            }
        }

        /// Arbitrary Unicode and arbitrary positions, including ones inside
        /// characters and far out of range: nothing panics, and every
        /// result stays in range.
        #[test]
        fn arbitrary_input_never_panics(
            text in any::<String>(),
            insert in any::<String>(),
            positions in prop::collection::vec(prop_oneof![0usize..64, Just(usize::MAX)], 6),
            tab_width in any::<u32>(),
        ) {
            let tab_width = TabWidth::new(tab_width);
            let mut rope = Rope::from(text.as_str());
            let mut map = DisplayMap::new(&rope, tab_width);
            let [a, b, c, d, e, f] = positions[..] else { unreachable!() };
            let edit = rope.replace(a..b, &insert);
            map.sync(&rope, &edit);
            let _ = rope.slice(c..d).to_string();
            let _ = rope.chunks_in_range(d..c).count();
            let _ = rope.lines().count();
            let point = rope.offset_to_point(e);
            prop_assert!(rope.point_to_offset(point) <= rope.len());
            prop_assert!(rope.point_to_offset(crate::rope::Point::new(e, f)) <= rope.len());
            prop_assert!(rope.line_end(f) <= rope.len());
            let snapshot = map.snapshot();
            let display = snapshot.offset_to_display_point(e);
            prop_assert!(display <= snapshot.max_point());
            for bias in [Bias::Left, Bias::Right] {
                let offset = snapshot.display_point_to_offset(DisplayPoint::new(e, f), bias);
                prop_assert!(offset <= rope.len());
                let _ = snapshot.clip_point(DisplayPoint::new(f, e), bias);
            }
            let _ = snapshot.row_cells(e);
            let _ = snapshot.clusters(DisplayPoint::new(c, d)).take(64).count();
            check_against(&snapshot, &rope.to_string(), tab_width);
        }

        #[test]
        fn progressive_loading_matches_building_at_once(
            pieces in prop::collection::vec(text_strategy(8), 0..15),
            tab_width in 1u32..10,
        ) {
            let tab_width = TabWidth::new(tab_width);
            let mut rope = Rope::new();
            let mut map = DisplayMap::new(&rope, tab_width);
            for piece in &pieces {
                let edit = rope.push_str(piece);
                map.sync(&rope, &edit);
            }
            check_against(&map.snapshot(), &pieces.concat(), tab_width);
        }
    }
}
