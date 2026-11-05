//! Placing shaped glyphs on the monospace cell grid (ADR 0005, ADR 0035,
//! ADR 0036).
//!
//! Must not depend on: the GPU, `atlas`, `shape` or `rows`.
//!
//! Cells come from the display model (`oso_core::display_map`), so a
//! cluster takes the same cells here as for cursor motion. The caller
//! lists the display clusters of the text it shaped; the text system only
//! says which cluster each glyph draws and where the font puts it.

use oso_core::display_map::CellStyle;
use oso_platform::text::{GlyphKey, ShapedGlyph};

/// The most glyphs drawn for one span. The display model bounds how many
/// characters a cluster holds, but a shaper's output for them has no such
/// bound, so it is cut off here rather than drawing it all in one cell.
const MAX_GLYPHS_PER_SPAN: u32 = 32;

/// A glyph placed on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlacedGlyph {
    pub(crate) key: GlyphKey,
    /// The first cell of the glyph's cluster, counted from the row start.
    pub(crate) column: usize,
    /// The cells the cluster takes: 1 for half-width, 2 for full-width,
    /// more for a ligature over several clusters. The glyph paints only
    /// these cells.
    pub(crate) cells: u32,
    /// Pen position relative to the cluster's first cell, at the baseline,
    /// in whole pixels; y grows downwards. Centers glyphs whose advance is
    /// not exactly `cells` cells, such as CJK from a fallback font.
    pub(crate) offset: [f32; 2],
    pub(crate) style: CellStyle,
}

/// A display cluster of the shaped text: where it starts in the text, and
/// its cells on the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grapheme {
    pub(crate) byte: u32,
    pub(crate) column: usize,
    pub(crate) cells: u32,
    pub(crate) style: CellStyle,
}

/// A run of display clusters that the shaper drew as one unit: usually one
/// cluster, several for a ligature.
#[derive(Debug, Clone, Copy)]
struct Span {
    column: usize,
    cells: u32,
    style: CellStyle,
    /// Glyphs placed so far.
    glyphs: u32,
    /// Leftmost pen position of the span's glyphs, and their total advance.
    x: f32,
    advance: f32,
}

/// Places shaped glyphs on the grid, reusing its buffers across rows.
#[derive(Debug, Default)]
pub(crate) struct Placer {
    /// Whether a glyph's cluster starts at each grapheme.
    starts: Vec<bool>,
    /// The span each grapheme belongs to.
    span_of: Vec<u32>,
    spans: Vec<Span>,
}

impl Placer {
    /// Places the glyphs of shaped text on a grid of cells `cell_width`
    /// pixels wide, replacing the contents of `out`. `graphemes` are the
    /// text's display clusters in order, starting at byte 0. The placed
    /// glyphs are sorted by column.
    ///
    /// A glyph's cluster is snapped to the display cluster that contains
    /// it, so a combining mark the shaper drew separately stays in its
    /// base's cell. A shaper cluster that covers several display clusters
    /// (a ligature) spans all their cells.
    pub(crate) fn place(
        &mut self,
        graphemes: &[Grapheme],
        glyphs: &[ShapedGlyph],
        cell_width: f32,
        out: &mut Vec<PlacedGlyph>,
    ) {
        out.clear();
        if glyphs.is_empty() || graphemes.is_empty() {
            return;
        }
        let grapheme_of = |byte: u32| {
            graphemes
                .partition_point(|g| g.byte <= byte)
                .saturating_sub(1)
        };

        // Which graphemes start a shaper cluster.
        self.starts.clear();
        self.starts.resize(graphemes.len(), false);
        for glyph in glyphs {
            if let Some(start) = self.starts.get_mut(grapheme_of(glyph.cluster)) {
                *start = true;
            }
        }

        // Group graphemes into spans, from one shaper cluster start to the
        // next, or to where the style changes: an escape's label keeps its
        // own cells whatever follows it.
        self.spans.clear();
        self.span_of.clear();
        for (i, g) in graphemes.iter().enumerate() {
            match self.spans.last_mut() {
                Some(span) if !self.starts[i] && span.style == g.style => {
                    span.cells = span.cells.saturating_add(g.cells);
                }
                _ => self.spans.push(Span {
                    column: g.column,
                    cells: g.cells,
                    style: g.style,
                    glyphs: 0,
                    x: f32::INFINITY,
                    advance: 0.0,
                }),
            }
            self.span_of.push(self.spans.len() as u32 - 1);
        }

        let span_index =
            |placer: &Self, byte: u32| placer.span_of.get(grapheme_of(byte)).map(|&s| s as usize);
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
                style: span.style,
            });
        }
        // Right-to-left runs come in visual order; drawing and lookups by
        // column need column order.
        if !out.is_sorted_by_key(|g| g.column) {
            out.sort_by_key(|g| g.column);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::display_map::cell::classify;
    use oso_core::display_map::{CellKind, Clusters, TabWidth};
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

    /// The display clusters of `text`, as the display model gives them.
    fn graphemes(text: &str) -> Vec<Grapheme> {
        let mut column = 0;
        Clusters::new(text)
            .map(|(byte, _, kind, cells)| {
                let cells = match kind {
                    CellKind::Tab => TabWidth::default().next_stop(column) - column,
                    _ => cells,
                };
                let g = Grapheme {
                    byte: byte as u32,
                    column,
                    cells: cells as u32,
                    style: CellStyle::Text,
                };
                column += cells;
                g
            })
            .collect()
    }

    fn place(text: &str, glyphs: &[ShapedGlyph]) -> Vec<PlacedGlyph> {
        let mut out = Vec::new();
        Placer::default().place(&graphemes(text), glyphs, CELL, &mut out);
        out
    }

    fn columns(text: &str) -> Vec<(usize, u32)> {
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
        // One cluster, however many glyphs the shaper gives for it.
        let cluster = Grapheme {
            byte: 0,
            column: 0,
            cells: 1,
            style: CellStyle::Text,
        };
        let mut placed = Vec::new();
        Placer::default().place(&[cluster], &shape_naively(&text), CELL, &mut placed);
        assert_eq!(placed.len(), MAX_GLYPHS_PER_SPAN as usize);
        assert!(placed.iter().all(|g| g.column == 0));
    }

    #[test]
    fn styles_follow_their_graphemes() {
        let text = "a^M";
        let mut graphemes = graphemes(text);
        graphemes[1].style = CellStyle::Escape;
        graphemes[2].style = CellStyle::Escape;
        let mut out = Vec::new();
        Placer::default().place(&graphemes, &shape_naively(text), CELL, &mut out);
        let styles: Vec<_> = out.iter().map(|g| g.style).collect();
        assert_eq!(
            styles,
            [CellStyle::Text, CellStyle::Escape, CellStyle::Escape]
        );
    }

    #[test]
    fn a_span_ends_where_the_style_changes() {
        // Text with no glyph of its own follows the last cell of a label.
        let text = "Ex";
        let mut graphemes = graphemes(text);
        graphemes[0].style = CellStyle::Boxed;
        let mut glyphs = shape_naively(text);
        glyphs.pop();
        let mut out = Vec::new();
        Placer::default().place(&graphemes, &glyphs, CELL, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cells, 1);
        assert_eq!(out[0].offset, [0.0, 0.0]);
    }

    #[test]
    fn graphemes_need_not_start_at_column_zero() {
        let text = "ab";
        let mut graphemes = graphemes(text);
        for g in &mut graphemes {
            g.column += 1_000_000;
        }
        let mut out = Vec::new();
        Placer::default().place(&graphemes, &shape_naively(text), CELL, &mut out);
        assert_eq!(out[1].column, 1_000_001);
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
                end = g.column + g.cells as usize;
                last = Some(g.column);
            }
        }
    }
}
