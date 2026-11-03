//! Placing shaped glyphs on the monospace cell grid (ADR 0005): half-width
//! clusters take one cell, full-width clusters two.
//!
//! The width table here is a minimal stand-in, approximating Unicode East
//! Asian Width and emoji presentation. It is to be consolidated with the
//! grapheme widths of the display model (M1-4) once that lands, so that
//! cursor motion and drawing agree on every cluster.

use super::{GlyphKey, ShapedGlyph};

/// Code point ranges that take no cell of their own: combining marks,
/// joiners and variation selectors. Sorted and disjoint.
const ZERO_WIDTH: &[(u32, u32)] = &[
    (0x0300, 0x036F),
    (0x0483, 0x0489),
    (0x0591, 0x05BD),
    (0x0610, 0x061A),
    (0x064B, 0x065F),
    (0x1160, 0x11FF),
    (0x1AB0, 0x1AFF),
    (0x1DC0, 0x1DFF),
    (0x200B, 0x200D),
    (0x20D0, 0x20FF),
    (0x3099, 0x309A),
    (0xFE00, 0xFE0F),
    (0xFE20, 0xFE2F),
    (0xE0100, 0xE01EF),
];

/// Code point ranges that take two cells: East Asian Wide and Fullwidth,
/// and emoji shown as emoji by default. Sorted and disjoint.
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F),
    (0x231A, 0x231B),
    (0x2329, 0x232A),
    (0x23E9, 0x23EC),
    (0x23F0, 0x23F0),
    (0x23F3, 0x23F3),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x2E80, 0x303E),
    (0x3041, 0x3098),
    (0x309B, 0x33FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xA000, 0xA4CF),
    (0xA960, 0xA97F),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE10, 0xFE19),
    (0xFE30, 0xFE6F),
    (0xFF00, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x16FE0, 0x16FE4),
    (0x17000, 0x18AFF),
    (0x1B000, 0x1B2FF),
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F200, 0x1F2FF),
    (0x1F300, 0x1F64F),
    (0x1F680, 0x1F6FF),
    (0x1F7E0, 0x1F7EB),
    (0x1F900, 0x1F9FF),
    (0x1FA70, 0x1FAFF),
    (0x20000, 0x2FFFD),
    (0x30000, 0x3FFFD),
];

/// Variation selector 16: asks for emoji presentation, which is full-width.
const EMOJI_PRESENTATION: char = '\u{FE0F}';

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let c = u32::from(c);
    let i = table.partition_point(|&(_, hi)| hi < c);
    table.get(i).is_some_and(|&(lo, _)| lo <= c)
}

/// The cells one character takes on its own: 0, 1 or 2.
pub fn char_cells(c: char) -> u8 {
    if in_table(ZERO_WIDTH, c) {
        0
    } else if in_table(WIDE, c) {
        2
    } else {
        1
    }
}

/// A glyph as a shaper produced it, before it is placed on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawGlyph {
    pub key: GlyphKey,
    /// UTF-16 offset in the text where the glyph's cluster starts, as
    /// CoreText and DirectWrite report it.
    pub utf16: usize,
    /// Pen position from the run origin, in pixels; y grows upwards.
    pub x: f32,
    pub y: f32,
    /// Horizontal advance in pixels.
    pub advance: f32,
}

/// One cluster of characters, as placed on the grid.
#[derive(Debug, Clone, Copy)]
struct Cluster {
    utf16: usize,
    byte: u32,
    column: u32,
    cells: u8,
    /// Leftmost pen position of the cluster's glyphs, and their total
    /// advance.
    x: f32,
    advance: f32,
}

/// Groups the characters of `text` into clusters.
///
/// A cluster starts where the shaper started one (`starts`, sorted), unless
/// that character takes no cell, such as a combining mark the shaper
/// returned separately: it joins the cluster before it.
fn clusters(text: &str, starts: &[usize]) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::with_capacity(starts.len());
    let mut next = 0;
    let mut utf16 = 0;
    let mut column = 0u32;
    // Cells of the current cluster: its characters' widths, and whether one
    // is full-width.
    let (mut narrow, mut wide) = (0u32, false);
    let finish = |c: &mut Cluster, narrow: u32, wide: bool| {
        c.cells = if wide {
            2
        } else {
            narrow.clamp(1, u32::from(u8::MAX)) as u8
        };
    };
    for (byte, c) in text.char_indices() {
        while next < starts.len() && starts[next] < utf16 {
            next += 1;
        }
        let shaper_start = starts.get(next) == Some(&utf16);
        let cells = char_cells(c);
        if clusters.is_empty() || (shaper_start && cells != 0) {
            if let Some(last) = clusters.last_mut() {
                finish(last, narrow, wide);
                column += u32::from(last.cells);
            }
            clusters.push(Cluster {
                utf16,
                byte: byte as u32,
                column,
                cells: 1,
                x: f32::INFINITY,
                advance: 0.0,
            });
            (narrow, wide) = (0, false);
        }
        wide |= cells == 2 || c == EMOJI_PRESENTATION;
        narrow += u32::from(cells);
        utf16 += c.len_utf16();
    }
    if let Some(last) = clusters.last_mut() {
        finish(last, narrow, wide);
    }
    clusters
}

/// Places the glyphs of a shaped run of `text` on a grid of cells
/// `cell_width` pixels wide, replacing the contents of `out`.
///
/// Each glyph keeps its position within its cluster, and the cluster is
/// centered in its cells, so that a full-width glyph from a fallback font
/// whose advance is not exactly two cells still sits in the middle of them.
pub fn place(text: &str, raw: &[RawGlyph], cell_width: f32, out: &mut Vec<ShapedGlyph>) {
    out.clear();
    if raw.is_empty() {
        return;
    }
    let mut starts: Vec<usize> = raw.iter().map(|g| g.utf16).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut clusters = clusters(text, &starts);
    let find = |clusters: &[Cluster], utf16: usize| {
        clusters
            .partition_point(|c| c.utf16 <= utf16)
            .saturating_sub(1)
    };
    for g in raw {
        let i = find(&clusters, g.utf16);
        if let Some(c) = clusters.get_mut(i) {
            c.x = c.x.min(g.x);
            c.advance += g.advance;
        }
    }
    out.reserve(raw.len());
    for g in raw {
        let Some(c) = clusters.get(find(&clusters, g.utf16)) else {
            continue;
        };
        let centering = (f32::from(c.cells) * cell_width - c.advance) / 2.0;
        out.push(ShapedGlyph {
            key: g.key,
            column: c.column,
            cells: c.cells,
            cluster: c.byte,
            offset: [(g.x - c.x + centering).round(), (-g.y).round()],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{FontId, GlyphId};

    const CELL: f32 = 10.0;

    /// One glyph per character, each advancing `advance` of the given cells.
    fn shape_naively(text: &str) -> Vec<RawGlyph> {
        let mut raw = Vec::new();
        let (mut utf16, mut x) = (0, 0.0);
        for (i, c) in text.chars().enumerate() {
            let advance = f32::from(char_cells(c)) * CELL;
            raw.push(RawGlyph {
                key: GlyphKey {
                    font: FontId(0),
                    glyph: GlyphId(i as u16),
                },
                utf16,
                x,
                y: 0.0,
                advance,
            });
            utf16 += c.len_utf16();
            x += advance;
        }
        raw
    }

    fn columns(text: &str) -> Vec<(u32, u8)> {
        let mut out = Vec::new();
        place(text, &shape_naively(text), CELL, &mut out);
        out.iter().map(|g| (g.column, g.cells)).collect()
    }

    #[test]
    fn widths_of_characters() {
        assert_eq!(char_cells('a'), 1);
        assert_eq!(char_cells('~'), 1);
        assert_eq!(char_cells('é'), 1);
        assert_eq!(char_cells('あ'), 2);
        assert_eq!(char_cells('日'), 2);
        assert_eq!(char_cells('\u{3000}'), 2, "ideographic space");
        assert_eq!(char_cells('Ａ'), 2, "fullwidth Latin");
        assert_eq!(char_cells('ｱ'), 1, "halfwidth katakana");
        assert_eq!(char_cells('🍣'), 2);
        assert_eq!(char_cells('✨'), 2);
        assert_eq!(char_cells('\u{301}'), 0, "combining acute");
        assert_eq!(char_cells('\u{200D}'), 0, "zero width joiner");
        assert_eq!(char_cells('\u{FE0F}'), 0, "variation selector");
    }

    #[test]
    fn tables_are_sorted_and_disjoint() {
        for table in [ZERO_WIDTH, WIDE] {
            for &(lo, hi) in table {
                assert!(lo <= hi, "{lo:#x}..{hi:#x}");
            }
            for pair in table.windows(2) {
                assert!(pair[0].1 < pair[1].0, "{pair:x?}");
            }
        }
        for &(lo, hi) in ZERO_WIDTH {
            for c in [lo, hi] {
                let c = char::from_u32(c).unwrap();
                assert!(!in_table(WIDE, c), "{c:?} is in both tables");
            }
        }
    }

    #[test]
    fn ascii_takes_one_cell_each() {
        assert_eq!(columns("abc"), [(0, 1), (1, 1), (2, 1)]);
    }

    #[test]
    fn full_width_takes_two_cells() {
        assert_eq!(columns("a日b"), [(0, 1), (1, 2), (3, 1)]);
    }

    #[test]
    fn emoji_outside_the_bmp() {
        // 🍣 is two UTF-16 units; the next glyph's cluster starts at 2.
        assert_eq!(columns("🍣x"), [(0, 2), (2, 1)]);
    }

    #[test]
    fn combining_mark_joins_its_base() {
        // The shaper returned the mark as a cluster of its own.
        assert_eq!(columns("e\u{301}x"), [(0, 1), (0, 1), (1, 1)]);
    }

    #[test]
    fn emoji_presentation_is_full_width() {
        // U+2764 is narrow on its own; with VS16 it is an emoji. The shaper
        // returns one glyph for both.
        let text = "\u{2764}\u{FE0F}x";
        let mut raw = shape_naively(text);
        raw.remove(1);
        let mut out = Vec::new();
        place(text, &raw, CELL, &mut out);
        let got: Vec<_> = out.iter().map(|g| (g.column, g.cells)).collect();
        assert_eq!(got, [(0, 2), (2, 1)]);
    }

    #[test]
    fn ligature_spans_its_characters() {
        // One glyph for "->": the cluster covers two narrow characters.
        let text = "->x";
        let mut raw = shape_naively(text);
        raw.remove(1);
        raw[0].advance = 2.0 * CELL;
        let mut out = Vec::new();
        place(text, &raw, CELL, &mut out);
        let got: Vec<_> = out.iter().map(|g| (g.column, g.cells)).collect();
        assert_eq!(got, [(0, 2), (2, 1)]);
    }

    #[test]
    fn cluster_byte_offsets() {
        let text = "a日🍣b";
        let mut out = Vec::new();
        place(text, &shape_naively(text), CELL, &mut out);
        let bytes: Vec<_> = out.iter().map(|g| g.cluster).collect();
        assert_eq!(bytes, [0, 1, 4, 8]);
    }

    #[test]
    fn narrow_fallback_glyph_is_centered() {
        // A full-width glyph whose advance is 16 px in 2 cells of 10 px.
        let text = "日";
        let mut raw = shape_naively(text);
        raw[0].advance = 16.0;
        let mut out = Vec::new();
        place(text, &raw, CELL, &mut out);
        assert_eq!(out[0].offset, [2.0, 0.0]);
    }

    #[test]
    fn glyph_in_a_cluster_keeps_its_offset() {
        // A base and a mark with zero advance, drawn 3 px to the right.
        let text = "e\u{301}";
        let mut raw = shape_naively(text);
        raw[0].advance = CELL;
        raw[1].x = 3.0;
        raw[1].y = 4.0;
        let mut out = Vec::new();
        place(text, &raw, CELL, &mut out);
        assert_eq!(out[0].offset, [0.0, 0.0]);
        assert_eq!(out[1].offset, [3.0, -4.0], "y grows downwards");
    }

    #[test]
    fn nothing_shaped_places_nothing() {
        let mut out = vec![];
        place("abc", &[], CELL, &mut out);
        assert!(out.is_empty());
    }

    /// Columns never decrease along the text, and each cluster starts
    /// where the one before it ends.
    #[test]
    fn columns_are_contiguous() {
        for text in [
            "",
            " ",
            "abc",
            "日本語",
            "a🍣b✨c",
            "e\u{301}\u{301}日",
            "\u{301}a",
            "🍣🍣 ab 日",
        ] {
            let mut out = Vec::new();
            place(text, &shape_naively(text), CELL, &mut out);
            let mut end = 0;
            let mut last = None;
            for g in &out {
                if last == Some(g.cluster) {
                    continue;
                }
                assert_eq!(g.column, end, "{text:?}");
                end = g.column + u32::from(g.cells);
                last = Some(g.cluster);
            }
        }
    }
}
