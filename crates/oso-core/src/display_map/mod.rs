//! The display model: maps buffer offsets to display rows and cell columns
//! and back, in O(log n) (`docs/dev/display-model.md`, ADR 0006,
//! ADR 0035).
//!
//! Must not depend on: `selection`, `buffer`, `window` or other editor state,
//! fonts or the GPU. It says what each cell shows; it does not paint it.
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
//! layer does not change their code. Text that is no buffer's has no layers, and
//! goes through [`text_clusters`].

pub mod cell;
pub mod tab_map;

use crate::decode::InvalidBytes;
use crate::rope::{Rope, RopeEdit};
use crate::sum_tree::Bias;

pub use cell::{CellKind, CellStyle, Clusters, Escape, MAX_CLUSTER_BYTES, TabWidth, escape};
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
    /// The buffer offset where the cluster starts, or the offset in the text
    /// given to [`text_clusters`].
    pub offset: usize,
    /// The first cell it takes.
    pub column: usize,
    /// How many cells it takes. A tab takes the cells up to its tab stop.
    pub cells: usize,
    /// The cluster's text in the buffer, or in the text given to
    /// [`text_clusters`].
    pub text: &'a str,
    /// What to draw: the text, blank cells for a tab, or an escape (boxed
    /// for an invisible character or one past a cluster's limits).
    pub kind: CellKind,
}

/// What one piece of a display cluster puts on the cell grid: its text in
/// `cells` cells from `column`. See [`DisplayCluster::for_each_run`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRun<'a> {
    /// The first cell it takes.
    pub column: usize,
    /// How many cells it takes.
    pub cells: usize,
    /// What to draw there. Never a control character or a bidi control.
    pub text: &'a str,
    /// How to draw it.
    pub style: CellStyle,
}

impl DisplayCluster<'_> {
    /// Calls `run` for what this cluster draws, in column order: its text,
    /// a space across a tab's cells, or an escape's label with one run per
    /// cell. Everything that draws the grid goes through here, so the
    /// renderer and the text-grid snapshots agree (ADR 0045).
    pub fn for_each_run(&self, mut run: impl FnMut(CellRun<'_>)) {
        let text = match self.kind {
            CellKind::Text => self.text,
            // A space keeps ligatures from forming across the tab, and
            // draws nothing.
            CellKind::Tab => " ",
            CellKind::LineBreak => return,
            CellKind::Control(_)
            | CellKind::Invisible(_)
            | CellKind::PastLimit(_)
            | CellKind::InvalidByte(_) => {
                let style = match self.kind {
                    CellKind::Invisible(_) | CellKind::PastLimit(_) => CellStyle::Boxed,
                    _ => CellStyle::Escape,
                };
                let escape = self.kind.escape();
                let label = escape.as_ref().map_or("?", Escape::as_str);
                for (index, c) in label.char_indices() {
                    run(CellRun {
                        column: self.column + index,
                        cells: 1,
                        text: &label[index..index + c.len_utf8()],
                        style,
                    });
                }
                return;
            }
        };
        run(CellRun {
            column: self.column,
            cells: self.cells,
            text,
            style: CellStyle::Text,
        });
    }
}

/// The clusters of `text` that is no buffer's, such as a modeline's, as one row
/// from `column`. A row of its own has no tab stops and no next line, so tabs and
/// line breaks are escapes like other control characters, and neither ends the row
/// early. Offsets are in `text`.
pub fn text_clusters(text: &str, mut column: usize) -> impl Iterator<Item = DisplayCluster<'_>> {
    let mut clusters = Clusters::new(text);
    // The line feed of a `\r\n`.
    let mut line_feed = None;
    std::iter::from_fn(move || {
        let (offset, text, kind, cells) = line_feed.take().or_else(|| clusters.next())?;
        let control = |c| {
            let label = escape(c).map_or(1, |escape| escape.as_str().len());
            (CellKind::Control(c), label)
        };
        let (text, (kind, cells)) = match kind {
            CellKind::Tab => (text, control('\t')),
            CellKind::LineBreak if text == "\r\n" => {
                line_feed = Some((offset + 1, &text[1..], kind, cells));
                (&text[..1], control('\r'))
            }
            CellKind::LineBreak => (text, control('\n')),
            _ => (text, (kind, cells)),
        };
        let cluster = DisplayCluster {
            offset,
            column,
            cells,
            text,
            kind,
        };
        column = column.saturating_add(cells);
        Some(cluster)
    })
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

    /// The display point of a buffer offset. An offset inside a display
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
pub(crate) mod tests {
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
            for (index, cluster, (kind, cells)) in pieces {
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

    /// A grapheme cluster split into display clusters, with offsets, kinds and
    /// cells.
    fn display_pieces(cluster: &str, offset: usize) -> Vec<(usize, &str, (CellKind, usize))> {
        let mut pieces = Vec::new();
        let mut start = 0;
        let mut past_limit = false;
        while start < cluster.len() {
            let rest = &cluster[start..];
            let (len, cell) = match rest.chars().next() {
                Some(c) if past_limit => (c.len_utf8(), cell::classify_past_limit(c)),
                _ => {
                    let first = cell::first_display_cluster(rest);
                    past_limit = first.at_limit;
                    (first.len, cell::classify(&rest[..first.len]))
                }
            };
            pieces.push((offset + start, &rest[..len], cell));
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

    pub(crate) fn check_against(snapshot: &DisplaySnapshot, text: &str, tab_width: TabWidth) {
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
        let inside_wide = DisplayPoint::new(1, 3);
        assert_eq!(snapshot.display_point_to_offset(inside_wide, Bias::Left), 7);
        assert_eq!(
            snapshot.display_point_to_offset(inside_wide, Bias::Right),
            10
        );
        assert_eq!(
            snapshot.clip_point(DisplayPoint::new(1, 1), Bias::Left),
            DisplayPoint::new(1, 0),
            "the middle of 日"
        );
    }

    #[test]
    fn clusters_from_a_column_read_the_text_before_it() {
        // The tag character is split out, so the mark starts a display
        // cluster. Whether the ZWJ joins the emoji after it depends on the
        // emoji before the tag, which comes before that column.
        let text = "👍\u{e0041}\u{301}\u{200d}👧 x";
        let snapshot = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
        check_against(&snapshot, text, TabWidth::default());
    }

    #[test]
    fn mappings_hold_past_a_clusters_limits() {
        // 21 bytes, so the whole grapheme cluster stays in one test chunk.
        let text = format!("e{}x\nab", "\u{301}".repeat(10));
        let snapshot = DisplayMap::new(&Rope::from(text.as_str()), TabWidth::default()).snapshot();
        check_against(&snapshot, &text, TabWidth::default());
        assert_eq!(snapshot.row_cells(0), 1 + 2 * 6 + 1);
        let boxed = snapshot.clusters(DisplayPoint::new(0, 7)).next();
        assert_eq!(boxed.map(|c| c.column), Some(7));
        assert_eq!(
            boxed.map(|c| c.kind),
            Some(CellKind::PastLimit('\u{301}')),
            "a pass resumed past the limit knows it"
        );
    }

    #[test]
    fn a_new_chunk_starts_a_clusters_limits_over() {
        let text = "e".to_owned() + &"\u{301}".repeat(crate::rope::MAX_CHUNK_BYTES * 4);
        let rope = Rope::from(text.as_str());
        let snapshot = DisplayMap::new(&rope, TabWidth::default()).snapshot();
        let stacks = snapshot
            .clusters(DisplayPoint::default())
            .filter(|c| c.kind == CellKind::Text)
            .count();
        assert_eq!(stacks, rope.chunks().count());
    }

    #[test]
    fn a_past_limit_character_draws_its_label_in_a_box() {
        let cluster = DisplayCluster {
            offset: 0,
            column: 2,
            cells: 6,
            text: "\u{301}",
            kind: CellKind::PastLimit('\u{301}'),
        };
        let labels: String = runs(&cluster).iter().map(|run| run.2.as_str()).collect();
        assert_eq!(labels, "U+0301");
        assert!(runs(&cluster).iter().all(|run| run.3 == CellStyle::Boxed));
        assert_eq!(runs(&cluster)[5].0, 7);
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

    /// What `text` draws as a row of its own, in order, as `(column, text)`.
    fn text_runs(text: &str, column: usize) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for cluster in text_clusters(text, column) {
            cluster.for_each_run(|run| out.push((run.column, run.text.to_owned())));
        }
        out
    }

    #[test]
    fn text_clusters_add_up_their_columns() {
        let clusters: Vec<_> = text_clusters("aあb", 3)
            .map(|c| (c.offset, c.column, c.cells, c.text.to_owned()))
            .collect();
        assert_eq!(
            clusters,
            [
                (0, 3, 1, "a".to_owned()),
                (1, 4, 2, "あ".to_owned()),
                (4, 6, 1, "b".to_owned()),
            ]
        );
    }

    #[test]
    fn line_breaks_and_tabs_in_text_clusters_are_escapes_and_hide_nothing() {
        let label =
            |text: &str| -> String { text_runs(text, 0).into_iter().map(|run| run.1).collect() };
        assert_eq!(label("a\nb"), "a^Jb");
        assert_eq!(label("a\r\nb"), "a^M^Jb");
        assert_eq!(label("a\tb"), "a^Ib");
        let columns: Vec<_> = text_runs("\r\nb", 0).into_iter().map(|run| run.0).collect();
        assert_eq!(columns, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn text_clusters_box_what_a_buffer_boxes() {
        let cluster = text_clusters("\u{202e}", 0).next().unwrap();
        assert_eq!(cluster.kind, CellKind::Invisible('\u{202e}'));
        assert_eq!(cluster.cells, 6);
    }

    fn runs(cluster: &DisplayCluster<'_>) -> Vec<(usize, usize, String, CellStyle)> {
        let mut runs = Vec::new();
        cluster.for_each_run(|run| {
            runs.push((run.column, run.cells, run.text.to_owned(), run.style));
        });
        runs
    }

    #[test]
    fn a_cluster_draws_its_text_a_blank_tab_or_a_label_per_cell() {
        let text = "日\t\r\u{202e}";
        let snapshot = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
        let drawn: Vec<_> = snapshot
            .clusters(DisplayPoint::new(0, 0))
            .flat_map(|cluster| runs(&cluster))
            .collect();
        let cell = |column, cells, text: &str, style| (column, cells, text.to_owned(), style);
        assert_eq!(
            drawn,
            [
                cell(0, 2, "日", CellStyle::Text),
                cell(2, 6, " ", CellStyle::Text),
                cell(8, 1, "^", CellStyle::Escape),
                cell(9, 1, "M", CellStyle::Escape),
                cell(10, 1, "U", CellStyle::Boxed),
                cell(11, 1, "+", CellStyle::Boxed),
                cell(12, 1, "2", CellStyle::Boxed),
                cell(13, 1, "0", CellStyle::Boxed),
                cell(14, 1, "2", CellStyle::Boxed),
                cell(15, 1, "E", CellStyle::Boxed),
            ]
        );
    }

    #[test]
    fn an_invalid_byte_draws_its_value_as_an_escape() {
        let cluster = DisplayCluster {
            offset: 0,
            column: 3,
            cells: 4,
            text: "\u{fffd}",
            kind: CellKind::InvalidByte(0xff),
        };
        let labels: String = runs(&cluster).iter().map(|run| run.2.as_str()).collect();
        assert_eq!(labels, "\\xFF");
        assert!(runs(&cluster).iter().all(|run| run.3 == CellStyle::Escape));
        assert_eq!(runs(&cluster)[3].0, 6);
    }

    proptest! {
        /// Whatever the text holds, no run draws a control character or a
        /// bidi control (T3 in security.md).
        #[test]
        fn runs_never_draw_controls(pieces in prop::collection::vec((0usize..14, 1usize..12), 0..40)) {
            const PIECES: [&str; 14] = [
                "a", "日", "\n", "\t", "\r", "\u{7f}", "\u{85}", "\u{202e}", "\u{2066}",
                "\u{200f}", "\u{61c}", "\u{301}", "\u{200d}", "👨\u{200d}👩",
            ];
            let text: String = pieces.into_iter().map(|(piece, n)| PIECES[piece].repeat(n)).collect();
            let snapshot = DisplayMap::new(&Rope::from(text.as_str()), TabWidth::default()).snapshot();
            for row in 0..snapshot.row_count() {
                for cluster in snapshot.clusters(DisplayPoint::new(row, 0)) {
                    for (_, _, drawn, _) in runs(&cluster) {
                        for c in drawn.chars() {
                            let bidi = matches!(u32::from(c), 0x61c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069);
                            prop_assert!(!c.is_control() && !bidi, "{c:?} in {text:?}");
                        }
                    }
                }
            }
        }

        /// A cluster's runs fill its cells exactly, in order.
        #[test]
        fn runs_cover_their_cluster(text in text_strategy(40)) {
            let snapshot = DisplayMap::new(&Rope::from(text.as_str()), TabWidth::default()).snapshot();
            for row in 0..snapshot.row_count() {
                for cluster in snapshot.clusters(DisplayPoint::new(row, 0)) {
                    let mut next = cluster.column;
                    for (column, cells, _, _) in runs(&cluster) {
                        prop_assert_eq!(column, next);
                        next += cells;
                    }
                    prop_assert_eq!(next, cluster.column + cluster.cells);
                }
            }
        }

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
