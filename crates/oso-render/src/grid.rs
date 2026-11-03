//! Placing shaped glyphs on the monospace cell grid (ADR 0005, ADR 0035,
//! ADR 0036).
//!
//! Cell widths come from the display model (`oso_core::display_map`), so a
//! cluster takes the same cells here as for cursor motion. The text system
//! only says which cluster each glyph draws and where the font puts it.

use oso_core::display_map::{CellKind, Clusters, TabWidth};
use oso_platform::text::{GlyphKey, ShapedGlyph};

/// The most glyphs drawn for one span. Real text stacks a few marks on a
/// letter; text stacking thousands ("Zalgo") is cut off here rather than
/// drawing them all in one cell.
const MAX_GLYPHS_PER_SPAN: u32 = 32;

/// A glyph placed on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlacedGlyph {
    pub(crate) key: GlyphKey,
    /// The first cell of the glyph's cluster, counted from the row start.
    pub(crate) column: u32,
    /// The cells the cluster takes: 1 for half-width, 2 for full-width,
    /// more for a ligature over several clusters.
    pub(crate) cells: u32,
    /// Pen position relative to the cluster's first cell, at the baseline,
    /// in whole pixels; y grows downwards. Centers glyphs whose advance is
    /// not exactly `cells` cells, such as CJK from a fallback font.
    pub(crate) offset: [f32; 2],
}

/// A display cluster of the row (`oso_core::display_map::Clusters`).
#[derive(Debug, Clone, Copy)]
struct Grapheme {
    byte: u32,
    column: u32,
}

/// A run of display clusters that the shaper drew as one unit: usually one
/// cluster, several for a ligature.
#[derive(Debug, Clone, Copy)]
struct Span {
    column: u32,
    cells: u32,
    /// Glyphs placed so far.
    glyphs: u32,
    /// Leftmost pen position of the span's glyphs, and their total advance.
    x: f32,
    advance: f32,
}

/// Places shaped glyphs on the grid, reusing its buffers across rows.
#[derive(Debug, Default)]
pub(crate) struct Placer {
    graphemes: Vec<Grapheme>,
    /// Whether a glyph's cluster starts at each grapheme.
    starts: Vec<bool>,
    /// The span each grapheme belongs to.
    span_of: Vec<u32>,
    spans: Vec<Span>,
}

impl Placer {
    /// Places the glyphs of a shaped row of `text` on a grid of cells
    /// `cell_width` pixels wide, replacing the contents of `out`. The placed
    /// glyphs are sorted by column.
    ///
    /// A glyph's cluster is snapped to the display cluster that contains
    /// it, so a combining mark the shaper drew separately stays in its
    /// base's cell. A shaper cluster that covers several display clusters
    /// (a ligature) spans all their cells.
    pub(crate) fn place(
        &mut self,
        text: &str,
        glyphs: &[ShapedGlyph],
        cell_width: f32,
        out: &mut Vec<PlacedGlyph>,
    ) {
        out.clear();
        if glyphs.is_empty() {
            return;
        }
        let end = self.measure(text);
        let grapheme_of = |graphemes: &[Grapheme], byte: u32| {
            graphemes
                .partition_point(|g| g.byte <= byte)
                .saturating_sub(1)
        };

        // Which graphemes start a shaper cluster.
        self.starts.clear();
        self.starts.resize(self.graphemes.len(), false);
        for glyph in glyphs {
            let i = grapheme_of(&self.graphemes, glyph.cluster);
            if let Some(start) = self.starts.get_mut(i) {
                *start = true;
            }
        }

        // Group graphemes into spans, from one shaper cluster start to the
        // next.
        self.spans.clear();
        self.span_of.clear();
        for (i, g) in self.graphemes.iter().enumerate() {
            if self.spans.is_empty() || self.starts[i] {
                if let Some(last) = self.spans.last_mut() {
                    last.cells = g.column - last.column;
                }
                self.spans.push(Span {
                    column: g.column,
                    cells: 0,
                    glyphs: 0,
                    x: f32::INFINITY,
                    advance: 0.0,
                });
            }
            self.span_of.push(self.spans.len() as u32 - 1);
        }
        if let Some(last) = self.spans.last_mut() {
            last.cells = end - last.column;
        }

        let span_index = |placer: &Self, byte: u32| {
            let g = grapheme_of(&placer.graphemes, byte);
            placer.span_of.get(g).map(|&s| s as usize)
        };
        for glyph in glyphs {
            if let Some(s) = span_index(self, glyph.cluster) {
                let span = &mut self.spans[s];
                span.x = span.x.min(glyph.x);
                span.advance += glyph.advance;
            }
        }
        out.reserve(glyphs.len());
        for glyph in glyphs {
            let Some(s) = span_index(self, glyph.cluster) else {
                continue;
            };
            let span = &mut self.spans[s];
            if span.glyphs >= MAX_GLYPHS_PER_SPAN {
                continue;
            }
            span.glyphs += 1;
            let span = *span;
            let cells = span.cells.max(1);
            let centering = (cells as f32 * cell_width - span.advance) / 2.0;
            out.push(PlacedGlyph {
                key: glyph.key,
                column: span.column,
                cells,
                offset: [(glyph.x - span.x + centering).round(), (-glyph.y).round()],
            });
        }
        // Right-to-left runs come in visual order; drawing and lookups by
        // column need column order.
        out.sort_by_key(|g| g.column);
    }

    /// Splits `text` into display clusters and finds their columns.
    /// Returns the column after the last one.
    fn measure(&mut self, text: &str) -> u32 {
        self.graphemes.clear();
        let mut column = 0u32;
        for (byte, _, kind, cells) in Clusters::new(text) {
            self.graphemes.push(Grapheme {
                byte: byte as u32,
                column,
            });
            column = column.saturating_add(Self::cells(kind, cells, column));
        }
        column
    }

    /// The cells of one display cluster at `column`. Tabs reach the next
    /// stop at the default width, until rows come from the display model
    /// (M1-6).
    fn cells(kind: CellKind, cells: usize, column: u32) -> u32 {
        match kind {
            CellKind::Tab => {
                let stop = TabWidth::default().next_stop(column as usize);
                (stop - column as usize) as u32
            }
            _ => cells as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::display_map::cell::classify;
    use oso_platform::text::{FontId, GlyphId};

    const CELL: f32 = 10.0;

    /// One glyph per character, advancing by the character's own width.
    fn shape_naively(text: &str) -> Vec<ShapedGlyph> {
        let mut glyphs = Vec::new();
        let mut x = 0.0;
        for (i, (byte, c)) in text.char_indices().enumerate() {
            let mut buf = [0; 4];
            let cells = match classify(c.encode_utf8(&mut buf)) {
                (CellKind::Text, _) if is_mark(c) => 0,
                (_, cells) => cells,
            };
            let advance = cells as f32 * CELL;
            glyphs.push(ShapedGlyph {
                key: GlyphKey {
                    font: FontId(0),
                    glyph: GlyphId(i as u16),
                },
                cluster: byte as u32,
                x,
                y: 0.0,
                advance,
            });
            x += advance;
        }
        glyphs
    }

    fn is_mark(c: char) -> bool {
        matches!(u32::from(c), 0x300..=0x36f)
    }

    fn place(text: &str, glyphs: &[ShapedGlyph]) -> Vec<PlacedGlyph> {
        let mut out = Vec::new();
        Placer::default().place(text, glyphs, CELL, &mut out);
        out
    }

    fn columns(text: &str) -> Vec<(u32, u32)> {
        place(text, &shape_naively(text))
            .iter()
            .map(|g| (g.column, g.cells))
            .collect()
    }

    #[test]
    fn ascii_takes_one_cell_each() {
        assert_eq!(columns("abc"), [(0, 1), (1, 1), (2, 1)]);
    }

    #[test]
    fn full_width_takes_two_cells() {
        assert_eq!(columns("a日b"), [(0, 1), (1, 2), (3, 1)]);
        assert_eq!(columns("ｱＡ"), [(0, 1), (1, 2)], "half and full width");
    }

    #[test]
    fn emoji_takes_two_cells() {
        assert_eq!(columns("🍣x"), [(0, 2), (2, 1)]);
    }

    #[test]
    fn combining_mark_stays_in_its_base_cell() {
        // The shaper returned the mark as a glyph of its own.
        assert_eq!(columns("e\u{301}x"), [(0, 1), (0, 1), (1, 1)]);
    }

    #[test]
    fn emoji_sequence_shaped_as_one_glyph() {
        // ❤️ is U+2764 and VS16: one display cluster of 2 cells.
        let text = "\u{2764}\u{FE0F}x";
        let mut glyphs = shape_naively(text);
        glyphs.remove(1);
        let got: Vec<_> = place(text, &glyphs)
            .iter()
            .map(|g| (g.column, g.cells))
            .collect();
        assert_eq!(got, [(0, 2), (2, 1)]);
    }

    #[test]
    fn ligature_spans_its_clusters() {
        // One glyph for "->": the shaper cluster covers two display
        // clusters.
        let text = "->x";
        let mut glyphs = shape_naively(text);
        glyphs.remove(1);
        glyphs[0].advance = 2.0 * CELL;
        let got: Vec<_> = place(text, &glyphs)
            .iter()
            .map(|g| (g.column, g.cells))
            .collect();
        assert_eq!(got, [(0, 2), (2, 1)]);
    }

    #[test]
    fn tab_reaches_the_next_stop() {
        assert_eq!(columns("a\tb"), [(0, 1), (1, 7), (8, 1)]);
    }

    #[test]
    fn narrow_fallback_glyph_is_centered() {
        // A full-width glyph whose advance is 16 px in 2 cells of 10 px.
        let mut glyphs = shape_naively("日");
        glyphs[0].advance = 16.0;
        assert_eq!(place("日", &glyphs)[0].offset, [2.0, 0.0]);
    }

    #[test]
    fn glyph_in_a_cluster_keeps_its_offset() {
        // A base and a mark with zero advance, drawn 3 px right and 4 px up.
        let text = "e\u{301}";
        let mut glyphs = shape_naively(text);
        glyphs[1].x = 3.0;
        glyphs[1].y = 4.0;
        let placed = place(text, &glyphs);
        assert_eq!(placed[0].offset, [0.0, 0.0]);
        assert_eq!(placed[1].offset, [3.0, -4.0], "y grows downwards");
    }

    #[test]
    fn right_to_left_glyphs_are_sorted_by_column() {
        // Hebrew comes from the shaper in visual order, right to left.
        let text = "אבג";
        let mut glyphs = shape_naively(text);
        glyphs.reverse();
        let columns: Vec<_> = place(text, &glyphs).iter().map(|g| g.column).collect();
        assert_eq!(columns, [0, 1, 2]);
    }

    #[test]
    fn stacked_marks_are_capped() {
        let text: String = std::iter::once('e')
            .chain(std::iter::repeat_n('\u{301}', 10_000))
            .collect();
        let placed = place(&text, &shape_naively(&text));
        assert_eq!(placed.len(), MAX_GLYPHS_PER_SPAN as usize);
        assert!(placed.iter().all(|g| g.column == 0));
    }

    #[test]
    fn nothing_shaped_places_nothing() {
        assert!(place("abc", &[]).is_empty());
    }

    /// Each cluster starts where the one before it ends.
    #[test]
    fn columns_are_contiguous() {
        for text in [
            " ",
            "abc",
            "日本語",
            "a🍣b✨c",
            "e\u{301}\u{301}日",
            "\u{301}a",
            "🍣🍣 ab 日",
            "👨\u{200d}👩\u{200d}👧!",
        ] {
            let placed = place(text, &shape_naively(text));
            let mut end = 0;
            let mut last = None;
            for g in &placed {
                if last == Some(g.column) {
                    continue;
                }
                assert_eq!(g.column, end, "{text:?}");
                end = g.column + g.cells;
                last = Some(g.column);
            }
        }
    }
}
