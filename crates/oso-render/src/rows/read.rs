//! What a row is read into before it is shaped: the clusters of a buffer's row or of spans, the
//! pieces they are cut into, and their boxes.
//!
//! Must not depend on: the GPU or `text`.

use std::ops::{ControlFlow, Range};

use oso_core::display_map::{
    CellStyle, DisplayCluster, DisplayPoint, DisplaySnapshot, MAX_CLUSTER_BYTES, Preedit,
    row_clusters, text_clusters,
};

use super::{CHARS_PER_COLUMN, Rows};
use crate::grid::Grapheme;
use crate::pane::{Face, Span};

/// The part of `text` that can show in `columns` columns: a display cluster takes
/// at least one cell. A span is cut to this before it is read, since nothing past
/// it can show.
pub(super) fn visible_part(text: &str, columns: usize) -> &str {
    let bytes = columns.saturating_mul(MAX_CLUSTER_BYTES);
    &text[..text.floor_char_boundary(bytes)]
}

/// What a view row that continues ends with (ADR 0100).
const WRAP_MARK: &str = "\u{21a9}";

/// The first character of `text`.
fn first_char(text: &str) -> &str {
    text.chars().next().map_or("", |c| &text[..c.len_utf8()])
}

impl Rows {
    /// Reads the clusters of `row` that start in `columns`, and no further,
    /// into the scratch space: what to shape, and the boxes to draw. A `preedit`
    /// goes in at its column, and what follows it moves right.
    pub(super) fn read(
        &mut self,
        display: &DisplaySnapshot,
        row: usize,
        columns: &Range<usize>,
        preedit: Option<(usize, &Preedit)>,
    ) {
        self.clear_scratch();
        let budget = columns.len().saturating_mul(CHARS_PER_COLUMN);
        let mut spent = 0;
        let mut face = None;
        let start = DisplayPoint::new(row, columns.start);
        // From the first cluster, which may start before the first column.
        self.base = columns.start;
        let mut first = true;
        row_clusters(display, start, preedit, self.whitespace, |cluster, own| {
            if cluster.column >= columns.end {
                return ControlFlow::Break(());
            }
            if std::mem::take(&mut first) {
                self.base = cluster.column.min(columns.start);
            }
            let first = self.graphemes.len();
            if !own {
                self.clusters
                    .push((cluster.column.saturating_sub(self.base), cluster.offset));
            }
            let shown = if own { Face::Preedit } else { Face::Text };
            let cut = self.push_cluster(&cluster, shown, budget, &mut spent);
            self.break_at(first, cut, &mut face);
            ControlFlow::Continue(())
        });
    }

    /// Reads the clusters of `spans` that start before column `end`, and no
    /// further, into the scratch space. Each span is segmented on its own, and
    /// its columns follow the last one's, and has its own budget of characters.
    /// A piece to shape starts at each span and wherever the face changes.
    pub(super) fn read_line(&mut self, spans: &[Span<'_>], end: usize) {
        self.clear_scratch();
        self.base = 0;
        let budget = end.saturating_mul(CHARS_PER_COLUMN);
        let mut column = 0;
        for span in spans {
            let mut spent = 0;
            let mut face = None;
            for cluster in text_clusters(visible_part(span.text, end), column) {
                if cluster.column >= end {
                    return;
                }
                column = cluster.column.saturating_add(cluster.cells);
                let first = self.graphemes.len();
                let cut = self.push_cluster(&cluster, span.face, budget, &mut spent);
                self.break_at(first, cut, &mut face);
            }
        }
    }

    /// Adds the wrap mark at `column`, a piece of its own (ADR 0100).
    pub(super) fn push_mark(&mut self, column: usize) {
        let byte = u32::try_from(self.text.len()).unwrap_or(u32::MAX);
        self.breaks.push(byte);
        self.graphemes.push(Grapheme {
            byte,
            column: column.saturating_sub(self.base),
            cells: 1,
            face: Face::WrapMark,
        });
        self.text.push_str(WRAP_MARK);
    }

    /// Starts a piece to shape wherever the graphemes from `first` on change from
    /// `face`, the last one's. A cluster that was `cut` is a piece of its own, so
    /// nothing forms with what is around it.
    fn break_at(&mut self, first: usize, cut: bool, face: &mut Option<Face>) {
        if cut {
            *face = None;
        }
        for grapheme in &self.graphemes[first..] {
            if *face != Some(grapheme.face) {
                *face = Some(grapheme.face);
                self.breaks.push(grapheme.byte);
            }
        }
        if cut {
            *face = None;
        }
    }

    pub(super) fn clear_scratch(&mut self) {
        self.text.clear();
        self.graphemes.clear();
        self.breaks.clear();
        self.boxes.clear();
        self.clusters.clear();
        self.looks.clear();
        self.cut_end = None;
    }

    /// Adds what `cluster` draws to the scratch space, with its text in
    /// `face`. Past `budget` characters, a run of several characters is cut
    /// to its first one and boxed, and this returns true. A label comes a
    /// character a run, so it is never cut.
    fn push_cluster(
        &mut self,
        cluster: &DisplayCluster<'_>,
        face: Face,
        budget: usize,
        spent: &mut usize,
    ) -> bool {
        let whole = *spent < budget;
        let mut cut = false;
        let base = self.base;
        let cluster_column = cluster.column.saturating_sub(base);
        cluster.for_each_run(|run| {
            // From the row's first cluster (see `ShapedRow::base`).
            let column = run.column.saturating_sub(base);
            let mut face = face.of_run(run.style);
            let mut text = run.text;
            if !whole && first_char(run.text).len() < run.text.len() {
                text = first_char(run.text);
                face = Face::Boxed;
                cut = true;
                // Cut clusters side by side share a box.
                match self.boxes.last_mut() {
                    Some((first, cells)) if self.cut_end == Some(column) => {
                        *cells = column + run.cells - *first;
                    }
                    _ => self.boxes.push((column, run.cells)),
                }
                self.cut_end = Some(column + run.cells);
            }
            // One box per boxed character, across its label's runs.
            if run.style == CellStyle::Boxed {
                if column == cluster_column {
                    self.boxes.push((column, run.cells));
                    self.cut_end = None;
                } else if let Some((_, cells)) = self.boxes.last_mut() {
                    *cells += run.cells;
                }
            }
            self.graphemes.push(Grapheme {
                byte: self.text.len() as u32,
                column,
                cells: u32::try_from(run.cells).unwrap_or(u32::MAX),
                face,
            });
            *spent += text.chars().count();
            self.text.push_str(text);
        });
        cut
    }
}
