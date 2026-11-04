//! The shaping cache: rows of buffer text shaped through the text system
//! and placed on the cell grid, by display row (ADR 0040).
//!
//! Must not depend on: the GPU or `text`, which turns rows into quads.
//!
//! - A row is shaped only for the columns asked for, so a 10 MB line costs
//!   one screen.
//! - A shaped row is reused without looking at the buffer while the
//!   buffer's revision holds, and after a change while the buffer offsets
//!   its columns cover are the same.
//! - Control characters, invalid bytes and invisible characters are never
//!   shaped. Their escapes from the display model (`^M`, `\xFF`,
//!   `U+202E`) are shaped instead, as plain ASCII labels (security.md, T3),
//!   and invisible characters get a box.

use std::collections::HashMap;
use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::display_map::{CellStyle, DisplayPoint, DisplaySnapshot};
use oso_core::sum_tree::Bias;
use oso_platform::text::{ShapedGlyph, TextSystem};

use crate::grid::{Grapheme, PlacedGlyph, Placer};

/// The most characters of one cluster passed to the shaper: enough for
/// the longest emoji sequences and Indic clusters. A cluster of a base and
/// thousands of combining marks takes one cell; shaping all of it on the
/// UI thread would let a small file stall a frame.
const MAX_CLUSTER_CHARS: usize = 16;

/// The characters shaped per row, per column shaped. Real text has one or
/// two; past this budget, clusters are shaped by their first character
/// only, so hostile text costs a bounded amount per row.
const CHARS_PER_COLUMN: usize = 4;

/// `text` cut to its first `chars` characters.
fn clip_cluster(text: &str, chars: usize) -> &str {
    match text.char_indices().nth(chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// One display row's shaped columns.
#[derive(Debug, Default)]
pub(crate) struct ShapedRow {
    /// The columns shaped.
    columns: Range<usize>,
    /// The buffer offsets those columns cover. Text loaded or edited there
    /// changes them, which makes the row shape again.
    offsets: Range<usize>,
    /// The buffer revision the offsets were checked at. While it holds,
    /// the row is reused without looking at the buffer.
    revision: u64,
    /// Sorted by column.
    pub(crate) glyphs: Vec<PlacedGlyph>,
    /// Boxed invisible characters: their first column and cells.
    pub(crate) boxes: Vec<(usize, usize)>,
}

/// Shaped rows of one buffer, by display row: the shaping cache.
#[derive(Default)]
pub(crate) struct Rows {
    buffer: Option<BufferId>,
    /// The revision of the buffer this frame shows.
    revision: u64,
    rows: HashMap<usize, ShapedRow>,
    /// Scratch space for shaping a row.
    text: String,
    graphemes: Vec<Grapheme>,
    glyphs: Vec<ShapedGlyph>,
    placer: Placer,
    /// Rows shaped since the cache was created, for tests and logs.
    shaped: u64,
}

impl Rows {
    /// Forgets every row's shaping, for when the font changes.
    pub(crate) fn invalidate(&mut self) {
        self.rows.clear();
    }

    /// Starts a frame of `buffer` at `revision`: another buffer's rows are
    /// dropped.
    pub(crate) fn begin(&mut self, buffer: BufferId, revision: u64) {
        if self.buffer != Some(buffer) {
            self.buffer = Some(buffer);
            self.rows.clear();
        }
        self.revision = revision;
    }

    /// Drops cached rows outside `keep` once there are many more of them
    /// than a screen.
    pub(crate) fn evict(&mut self, keep: Range<usize>) {
        if self.rows.len() > 2 * keep.len() + 64 {
            self.rows.retain(|row, _| keep.contains(row));
        }
    }

    /// Rows shaped so far.
    pub(crate) fn shaped_count(&self) -> u64 {
        self.shaped
    }

    /// The shaped `columns` of `row`, shaping them if the cache has none
    /// for the text there now.
    pub(crate) fn row(
        &mut self,
        display: &DisplaySnapshot,
        row: usize,
        columns: Range<usize>,
        system: &mut dyn TextSystem,
    ) -> &ShapedRow {
        let revision = self.revision;
        let unchanged = self
            .rows
            .get(&row)
            .is_some_and(|shaped| shaped.revision == revision && shaped.columns == columns);
        if unchanged {
            return &self.rows[&row];
        }
        let offsets = display
            .display_point_to_offset(DisplayPoint::new(row, columns.start), Bias::Left)
            ..display.display_point_to_offset(DisplayPoint::new(row, columns.end), Bias::Right);
        let fresh = self
            .rows
            .get(&row)
            .is_some_and(|shaped| shaped.columns == columns && shaped.offsets == offsets);
        if !fresh {
            let mut shaped = self.rows.remove(&row).unwrap_or_default();
            self.shape(display, row, &columns, system, &mut shaped);
            shaped.columns = columns;
            shaped.offsets = offsets;
            self.rows.insert(row, shaped);
        }
        let shaped = self.rows.get_mut(&row).expect("inserted above");
        shaped.revision = revision;
        shaped
    }

    /// Shapes the clusters of `row` that start in `columns` into `out`.
    fn shape(
        &mut self,
        display: &DisplaySnapshot,
        row: usize,
        columns: &Range<usize>,
        system: &mut dyn TextSystem,
        out: &mut ShapedRow,
    ) {
        self.shaped += 1;
        self.text.clear();
        self.graphemes.clear();
        out.boxes.clear();
        let budget = columns.len().saturating_mul(CHARS_PER_COLUMN);
        for cluster in display.clusters(DisplayPoint::new(row, columns.start)) {
            if cluster.column >= columns.end {
                break;
            }
            let chars = if self.text.len() < budget {
                MAX_CLUSTER_CHARS
            } else {
                1
            };
            cluster.for_each_run(|run| {
                if run.style == CellStyle::Boxed {
                    match out.boxes.last_mut() {
                        Some((column, cells)) if *column + *cells == run.column => {
                            *cells += run.cells;
                        }
                        _ => out.boxes.push((run.column, run.cells)),
                    }
                }
                self.graphemes.push(Grapheme {
                    byte: self.text.len() as u32,
                    column: run.column,
                    cells: u32::try_from(run.cells).unwrap_or(u32::MAX),
                    style: run.style,
                });
                self.text.push_str(clip_cluster(run.text, chars));
            });
        }
        system.shape(&self.text, &mut self.glyphs);
        self.placer.place(
            &self.graphemes,
            &self.glyphs,
            system.metrics().width,
            &mut out.glyphs,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::buffer::Buffer;
    use oso_platform::text::{CellMetrics, FontId, GlyphBitmap, GlyphId, GlyphKey};

    /// A text system with one glyph per character, one cell wide, that
    /// records what it shaped.
    #[derive(Default)]
    struct Recorder {
        shaped: Vec<String>,
    }

    impl TextSystem for Recorder {
        fn metrics(&self) -> CellMetrics {
            CellMetrics {
                width: 10.0,
                height: 20.0,
                ascent: 16.0,
                descent: 4.0,
            }
        }

        fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
            self.shaped.push(text.to_owned());
            out.clear();
            for (i, (byte, _)) in text.char_indices().enumerate() {
                out.push(ShapedGlyph {
                    key: GlyphKey {
                        font: FontId(0),
                        glyph: GlyphId(i as u16),
                    },
                    cluster: byte as u32,
                    x: i as f32 * 10.0,
                    y: 0.0,
                    advance: 10.0,
                });
            }
        }

        fn rasterize(&mut self, _key: GlyphKey) -> Option<GlyphBitmap> {
            None
        }
    }

    #[test]
    fn escapes_are_shaped_as_labels_never_as_raw_characters() {
        let buffer = Buffer::from_bytes("t", b"a\xe2\x80\xaeb\r\x07\xff\tc");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, &mut system);
        assert_eq!(system.shaped, ["aU+202Eb^M^G\\xFF c"]);
        assert_eq!(shaped.boxes, [(1, 6)]);
        let styles: Vec<_> = shaped.glyphs.iter().map(|g| (g.column, g.style)).collect();
        assert_eq!(styles[0], (0, CellStyle::Text));
        assert_eq!(styles[1], (1, CellStyle::Boxed));
        assert_eq!(styles[7], (7, CellStyle::Text));
        assert_eq!(styles[8], (8, CellStyle::Escape));
        assert_eq!(styles[12], (12, CellStyle::Escape), "the invalid byte");
        assert_eq!(styles[16], (16, CellStyle::Text), "the tab");
        assert_eq!(styles[17], (24, CellStyle::Text), "after the tab stop");
    }

    #[test]
    fn neighboring_invisible_characters_share_a_box() {
        let buffer = Buffer::from_text("t", "\u{202e}\u{202e}a\u{200b}");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, &mut system);
        assert_eq!(shaped.boxes, [(0, 12), (13, 6)]);
    }

    #[test]
    fn rows_are_cached_by_line_and_reshaped_when_their_text_changes() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let buffer = Buffer::from_text("t", "one\ntwo\nthree");
        let display = buffer.display();
        let id = oso_core::editor::Editor::default().window().buffer();
        rows.begin(id, 1);
        rows.row(&display, 0, 0..128, &mut system);
        rows.row(&display, 1, 0..128, &mut system);
        rows.row(&display, 0, 0..128, &mut system);
        assert_eq!(rows.shaped_count(), 2, "row 0 came from the cache");
        // More text on row 2, as while loading, reshapes only row 2.
        rows.row(&display, 2, 0..128, &mut system);
        // The buffer changed: rows are checked against their offsets.
        rows.begin(id, 2);
        let longer = Buffer::from_text("t", "one\ntwo\nthree four");
        rows.row(&longer.display(), 2, 0..128, &mut system);
        rows.row(&longer.display(), 1, 0..128, &mut system);
        assert_eq!(rows.shaped_count(), 4);
        assert_eq!(system.shaped.last().unwrap(), "three four");
        // Other columns shape again.
        rows.row(&longer.display(), 1, 64..192, &mut system);
        assert_eq!(rows.shaped_count(), 5);
    }

    #[test]
    fn huge_clusters_are_cut_before_shaping() {
        let zalgo = format!("e{}", "\u{301}".repeat(100_000));
        let text = format!("{zalgo}\n{zalgo}");
        let buffer = Buffer::from_text("t", &text);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.row(&buffer.display(), 0, 0..256, &mut system);
        let shaped = system.shaped[0].chars().count();
        // The rope splits the cluster into 1 KB pieces of one cell each,
        // and each piece is cut to a few characters.
        assert!(
            shaped <= 256 * (CHARS_PER_COLUMN + 1),
            "{shaped} characters shaped"
        );
    }

    #[test]
    fn a_long_line_shapes_only_the_columns_asked_for() {
        let line = "あ".repeat(1_000_000);
        let buffer = Buffer::from_text("t", &line);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 640..768, &mut system);
        assert_eq!(system.shaped[0].chars().count(), 64);
        assert_eq!(shaped.glyphs[0].column, 640);
    }

    proptest::proptest! {
        /// Whatever the file holds, the shaper never sees a control
        /// character or a bidi control (T3 in security.md).
        #[test]
        fn prop_the_shaper_never_sees_controls(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200),
                                               extra in proptest::collection::vec(0usize..8, 0..20)) {
            let mut bytes = bytes;
            let specials = ["\u{202e}", "\u{2066}", "\u{200f}", "\u{61c}", "\r", "\u{85}", "\u{7f}", "👨\u{200d}👩"];
            for i in extra {
                bytes.extend_from_slice(specials[i].as_bytes());
            }
            let buffer = Buffer::from_bytes("t", &bytes);
            let display = buffer.display();
            let mut rows = Rows::default();
            let mut system = Recorder::default();
            for row in 0..display.row_count() {
                rows.row(&display, row, 0..256, &mut system);
            }
            for shaped in &system.shaped {
                for c in shaped.chars() {
                    let bidi = matches!(u32::from(c), 0x61c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069);
                    proptest::prop_assert!(!c.is_control() && !bidi, "{c:?} in {shaped:?}");
                }
            }
        }
    }
}
