//! Text an input method is composing, the preedit, and a display row's clusters
//! with it spliced in at the cursor (ADR 0069). It is no buffer's: nothing here
//! changes the display map, and the renderer and the text-grid snapshots both read
//! the cursor's row through [`row_clusters`], so they agree.
//!
//! Must not depend on: `tab_map`'s internals, or anything that knows where the
//! cursor is; the caller says where the preedit goes.
//!
//! The preedit's cells come from [`text_clusters`], so it takes the cells and
//! escapes it would take as text that is no buffer's. What follows it on the row
//! keeps its own cells, moved right by the preedit's width; a tab there keeps the
//! width of its buffer column.

use std::ops::{ControlFlow, Range};

use super::{CellKind, DisplayCluster, DisplayPoint, DisplaySnapshot, text_clusters};

/// The most characters of a preedit whose places are given to the input method, far
/// above any composition typed, so that one from another process costs a bounded list.
pub(crate) const MOST_PLACED_CHARS: usize = 4096;

/// One of a preedit's clusters, from the one pass that segments it, so that a row
/// read from any column gives the same clusters, and costs no segmenting.
#[derive(Clone, PartialEq, Eq)]
struct Cluster {
    /// Where its text is in the preedit's.
    bytes: Range<usize>,
    column: usize,
    cells: usize,
    kind: CellKind,
}

/// What an input method is composing, with the part it marks, in cells worked out
/// once. No `Debug`: it is text being typed (ADR 0028).
#[derive(Clone, PartialEq, Eq)]
pub struct Preedit {
    text: String,
    /// The input method's range, in bytes on character boundaries of `text`.
    range: Option<Range<usize>>,
    /// The cells `text` takes.
    cells: usize,
    /// Where the caret is, when `range` is a caret before the end: the cells
    /// before it, and those of the cluster it is on.
    caret: Option<(usize, usize)>,
    /// The cells of the segment being converted, when `range` is not empty.
    segment: Option<Range<usize>>,
    /// Its clusters, by column from its start.
    clusters: Vec<Cluster>,
}

impl Preedit {
    /// The preedit `text`, with `range` in bytes as the input method gives it: the
    /// caret if it is empty, else the segment being converted. A range off the
    /// text's character boundaries, or past its end, is dropped and the text kept.
    pub fn new(text: String, range: Option<Range<usize>>) -> Self {
        let range = range.filter(|range| {
            range.start <= range.end
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
        });
        let mut cells = 0;
        let mut caret = None;
        let mut segment: Option<Range<usize>> = None;
        let mut clusters = Vec::new();
        for cluster in text_clusters(&text, 0) {
            let end = cluster.offset + cluster.text.len();
            clusters.push(Cluster {
                bytes: cluster.offset..end,
                column: cluster.column,
                cells: cluster.cells,
                kind: cluster.kind,
            });
            let width = cluster.cells.max(1);
            if let Some(range) = &range {
                let covers = |byte: usize| cluster.offset <= byte && byte < end;
                if range.is_empty() && covers(range.start) {
                    caret = Some((cluster.column, width));
                }
                if !range.is_empty() && cluster.offset < range.end && range.start < end {
                    let start = segment.as_ref().map_or(cluster.column, |s| s.start);
                    segment = Some(start..cluster.column + width);
                }
            }
            cells = cluster.column + width;
        }
        Self {
            text,
            range,
            cells,
            caret,
            segment,
            clusters,
        }
    }

    /// The preedit's clusters from the one that covers `column` of its own, with
    /// columns from `at`: a search, and no segmenting.
    fn clusters_from(&self, at: usize, column: usize) -> impl Iterator<Item = DisplayCluster<'_>> {
        let first = self
            .clusters
            .partition_point(|cluster| cluster.column + cluster.cells.max(1) <= column);
        self.clusters[first..]
            .iter()
            .map(move |cluster| DisplayCluster {
                offset: cluster.bytes.start,
                column: at + cluster.column,
                cells: cluster.cells,
                text: &self.text[cluster.bytes.clone()],
                kind: cluster.kind,
                whitespace: false,
            })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The input method's range in bytes, if it gave one that fits the text.
    pub fn range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }

    /// The cells the preedit takes on its row.
    pub fn cells(&self) -> usize {
        self.cells
    }

    /// Where each of its characters is drawn, into `out`, emptied first: its x from the
    /// preedit's start, then that of its end, with cells `cell_width` pixels wide. A
    /// character takes its cluster's place. Nothing for a preedit of more than
    /// [`MOST_PLACED_CHARS`] characters, whose input method then gets the cursor's cell
    /// (ADR 0077). One walk of the clusters.
    pub(crate) fn char_positions(&self, cell_width: f32, out: &mut Vec<f32>) {
        out.clear();
        for cluster in &self.clusters {
            let x = cluster.column as f32 * cell_width;
            for _ in self.text[cluster.bytes.clone()].chars() {
                if out.len() == MOST_PLACED_CHARS {
                    out.clear();
                    return;
                }
                out.push(x);
            }
        }
        out.push(self.cells as f32 * cell_width);
    }

    /// The cells of the segment being converted, from the preedit's start.
    pub fn segment(&self) -> Option<Range<usize>> {
        self.segment.clone()
    }

    /// The columns its underline takes, with the preedit at column `at`, and those of
    /// the thicker one under the segment being converted.
    pub fn underlines(&self, at: usize) -> (Range<usize>, Option<Range<usize>>) {
        let segment = self.segment.as_ref().map(|s| at + s.start..at + s.end);
        (at..at + self.cells, segment)
    }

    /// Where the cursor box goes: the cells from the preedit's start, and the cells
    /// of the preedit's cluster there. `None` puts it after the preedit, on what
    /// follows, while typing at the end and while converting (ADR 0069).
    pub fn caret(&self) -> Option<(usize, usize)> {
        self.caret
    }
}

/// Calls `each` for the clusters of `start.row` from the one that covers
/// `start.column`, as [`DisplaySnapshot::clusters`] gives them, with `preedit`
/// spliced in at column `at` and what follows it moved right by its cells, until
/// `each` breaks. Each cluster comes with whether it is the preedit's, and a preedit
/// cluster's offset is in the preedit's text. `at` is where a cluster starts, as a
/// cursor is.
pub fn row_clusters<'a>(
    display: &'a DisplaySnapshot,
    start: DisplayPoint,
    preedit: Option<(usize, &'a Preedit)>,
    whitespace: bool,
    mut each: impl FnMut(DisplayCluster<'a>, bool) -> ControlFlow<()>,
) {
    // The buffer's clusters carry the whitespace marks if asked; a preedit's never do, as
    // it is drawn as it is composed (ADR 0097).
    let marked = |mut cluster: DisplayCluster<'a>| {
        cluster.whitespace = whitespace;
        cluster
    };
    let Some((at, preedit)) = preedit else {
        for cluster in display.clusters(start) {
            if each(marked(cluster), false).is_break() {
                return;
            }
        }
        return;
    };
    if start.column < at {
        for cluster in display.clusters(start) {
            if cluster.column >= at {
                break;
            }
            if each(marked(cluster), false).is_break() {
                return;
            }
        }
    }
    for cluster in preedit.clusters_from(at, start.column.saturating_sub(at)) {
        if cluster.column + cluster.cells.max(1) <= start.column {
            continue;
        }
        if each(cluster, true).is_break() {
            return;
        }
    }
    let width = preedit.cells;
    let from = start.column.saturating_sub(width).max(at);
    for mut cluster in display.clusters(DisplayPoint::new(start.row, from)) {
        cluster.column += width;
        if each(marked(cluster), false).is_break() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::{DisplayMap, TabWidth};
    use crate::rope::Rope;

    fn snapshot(text: &str) -> DisplaySnapshot {
        DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
    }

    /// The row's clusters from `start` as (column, text, whether the preedit's).
    fn row(
        display: &DisplaySnapshot,
        start: DisplayPoint,
        preedit: Option<(usize, &Preedit)>,
    ) -> Vec<(usize, String, bool)> {
        let mut out = Vec::new();
        row_clusters(display, start, preedit, false, |cluster, own| {
            out.push((cluster.column, cluster.text.to_owned(), own));
            ControlFlow::Continue(())
        });
        out.retain(|(_, text, _)| text != "\n");
        out
    }

    fn texts(row: &[(usize, String, bool)]) -> String {
        row.iter().map(|(_, text, _)| text.as_str()).collect()
    }

    #[test]
    fn a_character_is_drawn_in_its_clusters_place() {
        let mut out = vec![99.0];
        // A wide letter, an accent joined to the letter before it, an emoji of two cells.
        let preedit = Preedit::new("にe\u{301}😀a".into(), None);
        preedit.char_positions(8.0, &mut out);
        assert_eq!(out, [0.0, 16.0, 16.0, 24.0, 40.0, 48.0]);
        Preedit::new(String::new(), None).char_positions(8.0, &mut out);
        assert_eq!(out, [0.0], "only the end");
    }

    #[test]
    fn a_preedit_too_long_to_place_gets_no_places() {
        let mut out = Vec::new();
        let most = "a".repeat(MOST_PLACED_CHARS);
        Preedit::new(most.clone(), None).char_positions(1.0, &mut out);
        assert_eq!(out.len(), MOST_PLACED_CHARS + 1, "every one and the end");
        Preedit::new(most + "a", None).char_positions(1.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn the_preedit_takes_its_cells_from_the_clusters() {
        let preedit = Preedit::new("にほんa".into(), None);
        assert_eq!(preedit.cells(), 7);
        assert_eq!(preedit.caret(), None);
        assert_eq!(preedit.segment(), None);
        // A control character is its escape, as text that is no buffer's.
        assert_eq!(Preedit::new("a\u{1}".into(), None).cells(), 3);
    }

    #[test]
    fn a_caret_before_the_end_puts_the_box_on_its_cluster() {
        let text = "にほん".to_owned();
        assert_eq!(Preedit::new(text.clone(), Some(3..3)).caret(), Some((2, 2)));
        assert_eq!(Preedit::new(text.clone(), Some(9..9)).caret(), None);
        assert_eq!(Preedit::new(text, Some(0..0)).caret(), Some((0, 2)));
    }

    #[test]
    fn a_range_marks_the_segment_in_cells() {
        // 日本 then 語: the IME converts the second clause.
        let preedit = Preedit::new("日本語".into(), Some(6..9));
        assert_eq!(preedit.segment(), Some(4..6));
        assert_eq!(preedit.caret(), None);
        let emoji = Preedit::new("😀".into(), Some(0..4));
        assert_eq!(emoji.segment(), Some(0..2));
    }

    #[test]
    fn a_range_off_the_text_is_dropped_and_the_text_kept() {
        let backward = Range { start: 4, end: 2 };
        for range in [1..1, 0..1, backward, 0..10] {
            let preedit = Preedit::new("日本".into(), Some(range.clone()));
            assert_eq!(preedit.range(), None, "{range:?}");
            assert_eq!(preedit.text(), "日本");
            assert_eq!(preedit.cells(), 4);
        }
    }

    #[test]
    fn the_preedit_pushes_the_rest_of_the_row() {
        let display = snapshot("abcd\nnext\n");
        let preedit = Preedit::new("かな".into(), None);
        let spliced = row(&display, DisplayPoint::new(0, 0), Some((2, &preedit)));
        assert_eq!(texts(&spliced), "abかなcd");
        let columns: Vec<_> = spliced.iter().map(|(column, _, _)| *column).collect();
        assert_eq!(columns, [0, 1, 2, 4, 6, 7]);
        let own: Vec<_> = spliced.iter().map(|(_, _, own)| *own).collect();
        assert_eq!(own, [false, false, true, true, false, false]);
        assert_eq!(texts(&row(&display, DisplayPoint::new(0, 0), None)), "abcd");
    }

    #[test]
    fn a_preedit_at_the_end_of_the_row_comes_last() {
        let display = snapshot("ab");
        let preedit = Preedit::new("x".into(), None);
        let spliced = row(&display, DisplayPoint::new(0, 0), Some((2, &preedit)));
        assert_eq!(texts(&spliced), "abx");
    }

    #[test]
    fn a_row_read_from_a_later_column_starts_where_it_is_shown() {
        let display = snapshot("abcdef");
        let preedit = Preedit::new("XYZ".into(), None);
        // Past the preedit: the text after it, from its moved column.
        let late = row(&display, DisplayPoint::new(0, 6), Some((1, &preedit)));
        assert_eq!(texts(&late), "def");
        assert_eq!(late[0].0, 6);
        // Inside it: the rest of the preedit, then what it pushed.
        let inside = row(&display, DisplayPoint::new(0, 2), Some((1, &preedit)));
        assert_eq!(texts(&inside), "YZbcdef");
        // A cluster that ends at the start is left out, as it shows nothing.
        let wide = Preedit::new("日".into(), None);
        let after = row(&display, DisplayPoint::new(0, 3), Some((1, &wide)));
        assert_eq!(texts(&after), "bcdef");
    }

    #[test]
    fn a_long_preedit_is_read_from_near_the_column_asked_for() {
        let display = snapshot("ab");
        let preedit = Preedit::new("か".repeat(768), None);
        let start = DisplayPoint::new(0, 1 + 2 * 266);
        let late = row(&display, start, Some((1, &preedit)));
        assert_eq!(
            late[0].0, start.column,
            "the cluster at the column, and none before"
        );
        assert_eq!(late.len(), 768 - 266 + 1, "the rest of it, then b");
        assert_eq!(late.last().unwrap().1, "b");
    }

    #[test]
    fn any_column_reads_the_clusters_of_the_one_pass_even_past_a_clusters_limits() {
        // A letter with more marks than a display cluster takes, between others.
        let text = format!("か{}a\u{3000}b", "\u{301}".repeat(300));
        let display = snapshot("");
        let preedit = Preedit::new(text, None);
        let whole = row(&display, DisplayPoint::new(0, 0), Some((0, &preedit)));
        for column in 0..preedit.cells() {
            let from = row(&display, DisplayPoint::new(0, column), Some((0, &preedit)));
            // A cluster ends where the next starts, and the last at the preedit's end.
            let ends = whole.iter().skip(1).map(|(start, _, _)| *start);
            let expected: Vec<_> = whole
                .iter()
                .zip(ends.chain([preedit.cells()]))
                .filter(|(_, end)| *end > column)
                .map(|(cluster, _)| cluster.clone())
                .collect();
            assert_eq!(from.len(), expected.len(), "from column {column}");
            for (got, want) in from.iter().zip(&expected) {
                assert_eq!(got.0, want.0, "from column {column}");
                assert_eq!(got.1, want.1, "from column {column}");
            }
        }
    }

    #[test]
    fn a_break_before_the_preedit_ends_the_row() {
        let display = snapshot("abcdef");
        let preedit = Preedit::new("XY".into(), None);
        let mut seen = 0;
        row_clusters(
            &display,
            DisplayPoint::new(0, 0),
            Some((3, &preedit)),
            false,
            |_, _| {
                seen += 1;
                ControlFlow::Break(())
            },
        );
        assert_eq!(seen, 1);
    }

    #[test]
    fn each_can_stop_the_row() {
        let display = snapshot("abcdef");
        let preedit = Preedit::new("XY".into(), None);
        let mut seen = 0;
        row_clusters(
            &display,
            DisplayPoint::new(0, 0),
            Some((1, &preedit)),
            false,
            |_, _| {
                seen += 1;
                if seen == 2 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        );
        assert_eq!(seen, 2);
    }
}
