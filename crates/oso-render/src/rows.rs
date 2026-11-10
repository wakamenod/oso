//! The shaping cache: rows of buffer text shaped through the text system
//! and placed on the cell grid, by display row (ADR 0040).
//!
//! Must not depend on: the GPU or `text`, which turns rows into quads.
//!
//! - A row is shaped only for the columns asked for, so a 10 MB line costs
//!   one screen.
//! - A shaped row is reused without looking at the buffer while the
//!   buffer's revision holds. After a change, it is reused if what the
//!   shaper would be given for its columns is the same as last time, wherever
//!   that text now is in the buffer (ADR 0051). Rows are kept by display
//!   row, so scrolling reuses them and a row that moves to another display
//!   row is shaped again.
//! - Control characters, invalid bytes and invisible characters are never
//!   shaped. Their escapes from the display model (`^M`, `\xFF`,
//!   `U+202E`) are shaped instead, as plain ASCII labels (security.md, T3),
//!   and invisible characters get a box, as do characters past a display
//!   cluster's limits (ADR 0054) and clusters cut past the budget of
//!   characters (ADR 0068).
//! - Text that is no buffer's comes as spans, through the same clusters. Its
//!   row is compared with what was shaped each frame (ADR 0061).
//! - A row is shaped in pieces where the face changes, and at each span, so that
//!   nothing forms across them: text cannot change how a label or another span
//!   reads. A cluster cut past the budget is a piece of its own. A row of one
//!   piece is one call to the shaper, which may split it by direction (ADR 0066).
//!   Short pieces are kept by their text, so a label or a name is shaped once
//!   (ADR 0068).
//! - An input method's preedit is spliced into the cursor's row, in a face of its
//!   own, so it is shaped apart from the text around it. That row is read and
//!   compared each frame while it has one, and the frame after (ADR 0069).

use std::collections::HashMap;
use std::ops::{ControlFlow, Range};

use oso_core::buffer::BufferId;
use oso_core::display_map::{
    CellStyle, DisplayCluster, DisplayPoint, DisplaySnapshot, MAX_CLUSTER_BYTES, Preedit,
    row_clusters, text_clusters,
};
use oso_platform::text::{ShapedGlyph, TextSystem};

use crate::grid::{Grapheme, PlacedGlyph, Placer};
use crate::pane::{Face, Span};

/// The characters shaped per row, per column shaped. Real text has at most
/// three or so; past this budget, a cluster of several characters is shaped by
/// its first one, in a box, so that what is left out shows (ADR 0068). A row's
/// text then has at most this many characters per column plus one more, and
/// one cluster: hostile text costs a bounded amount per row, about 20 bytes per
/// column at worst. The display model bounds a cluster's length (ADR 0054).
const CHARS_PER_COLUMN: usize = 4;

/// The part of `text` that can show in `columns` columns: a display cluster takes
/// at least one cell. A span is cut to this before it is read, since nothing past
/// it can show.
fn visible_part(text: &str, columns: usize) -> &str {
    let bytes = columns.saturating_mul(MAX_CLUSTER_BYTES);
    &text[..text.floor_char_boundary(bytes)]
}

/// The longest piece of a row whose shaping is kept by its text, in bytes, and
/// how many are kept. A shaper's output depends only on its text and the font,
/// so a short piece, such as a label, a cut cluster's character or a name on
/// the modeline, is shaped once and not again for each row or frame it is in.
const KEPT_PIECE_BYTES: usize = 64;
const KEPT_PIECES: usize = 1024;

/// Shaped pieces of a row, by their text, for pieces of up to
/// [`KEPT_PIECE_BYTES`]. They hold buffer text, so they go with the rows they
/// came from and at each revision (ADR 0028). No `Debug` for the same reason.
#[derive(Default)]
struct Pieces {
    /// Where each piece's glyphs are in `shaped`.
    index: HashMap<Box<str>, usize>,
    shaped: Vec<Vec<ShapedGlyph>>,
    /// Scratch space for a piece that is too long to keep.
    long: Vec<ShapedGlyph>,
}

impl Pieces {
    /// The glyphs of `text` shaped alone.
    fn shape(&mut self, system: &mut dyn TextSystem, text: &str) -> &[ShapedGlyph] {
        if text.len() > KEPT_PIECE_BYTES {
            system.shape(text, &mut self.long);
            return &self.long;
        }
        if let Some(&at) = self.index.get(text) {
            return &self.shaped[at];
        }
        if self.shaped.len() >= KEPT_PIECES {
            self.clear();
        }
        let mut glyphs = Vec::new();
        system.shape(text, &mut glyphs);
        self.index.insert(text.into(), self.shaped.len());
        self.shaped.push(glyphs);
        &self.shaped[self.shaped.len() - 1]
    }

    fn clear(&mut self) {
        self.index.clear();
        self.shaped.clear();
    }
}

/// Shapes `text` into `out`. Each piece from a start in `breaks` is shaped on its
/// own, through `pieces`, and with no breaks the text is one piece.
fn shape_pieces(
    system: &mut dyn TextSystem,
    text: &str,
    breaks: &[u32],
    pieces: &mut Pieces,
    out: &mut Vec<ShapedGlyph>,
) {
    if breaks.is_empty() {
        system.shape(text, out);
        return;
    }
    out.clear();
    let mut pen = 0.0;
    for (index, &start) in breaks.iter().enumerate() {
        let end = breaks
            .get(index + 1)
            .map_or(text.len(), |&end| end as usize);
        let piece = pieces.shape(system, &text[start as usize..end]);
        for glyph in piece {
            out.push(ShapedGlyph {
                cluster: glyph.cluster + start,
                x: glyph.x + pen,
                ..*glyph
            });
        }
        pen += piece.iter().map(|glyph| glyph.advance).sum::<f32>();
    }
}

/// The first character of `text`.
fn first_char(text: &str) -> &str {
    text.chars().next().map_or("", |c| &text[..c.len_utf8()])
}

/// One display row's shaped columns. No `Debug`: it holds buffer text
/// (ADR 0028).
#[derive(Default)]
pub(crate) struct ShapedRow {
    /// The columns shaped.
    columns: Range<usize>,
    /// The buffer revision the row was checked at. While it holds, the row
    /// is reused without looking at the buffer.
    revision: u64,
    /// Whether it was read with a preedit, which the revision says nothing of.
    spliced: bool,
    /// What the shaper and the placer were given. The glyphs depend on
    /// nothing else but the font, so the same input gives the same glyphs,
    /// and comparing the bytes leaves no false match that could show stale
    /// text. Nothing here or in the glyphs is a buffer offset, so a row
    /// stays valid wherever its text moves in the buffer.
    text: String,
    graphemes: Vec<Grapheme>,
    /// Where the text was cut into pieces shaped on their own. Empty for a row
    /// of one piece, which is one call to the shaper.
    breaks: Vec<u32>,
    /// Sorted by column.
    pub(crate) glyphs: Vec<PlacedGlyph>,
    /// Boxed characters and cut clusters: their first column and cells.
    pub(crate) boxes: Vec<(usize, usize)>,
}

/// Shaped rows of one pane, by display row: the shaping cache. Each pane has
/// its own, because [`Rows::begin`] drops another buffer's rows.
#[derive(Default)]
pub(crate) struct Rows {
    buffer: Option<BufferId>,
    /// The revision of the buffer this frame shows.
    revision: u64,
    rows: HashMap<usize, ShapedRow>,
    /// Scratch space for reading a row.
    text: String,
    graphemes: Vec<Grapheme>,
    breaks: Vec<u32>,
    boxes: Vec<(usize, usize)>,
    /// Where the last box of a cut cluster ends, while it is the last box.
    cut_end: Option<usize>,
    glyphs: Vec<ShapedGlyph>,
    pieces: Pieces,
    placer: Placer,
    /// Rows shaped since the cache was created, for tests and logs.
    shaped: u64,
    /// Whether a buffer's rows mark whitespace (ADR 0097).
    whitespace: bool,
}

impl Rows {
    /// Forgets every row's shaping, for when the font changes.
    pub(crate) fn invalidate(&mut self) {
        self.rows.clear();
        self.pieces.clear();
    }

    /// Marks whitespace in a buffer's rows, or not. A change shapes every row again, as the
    /// revision says nothing of it.
    pub(crate) fn set_whitespace(&mut self, whitespace: bool) {
        if self.whitespace != whitespace {
            self.whitespace = whitespace;
            self.rows.clear();
            self.pieces.clear();
        }
    }

    /// Starts a frame of `buffer` at `revision`: another buffer's rows and
    /// kept pieces are dropped, and the pieces of another revision.
    pub(crate) fn begin(&mut self, buffer: BufferId, revision: u64) {
        if self.buffer != Some(buffer) {
            self.buffer = Some(buffer);
            self.rows.clear();
            self.pieces.clear();
        } else if self.revision != revision {
            self.pieces.clear();
        }
        self.revision = revision;
    }

    /// Starts a frame of text that is no buffer's. Rows left from a buffer are
    /// dropped, and so are these when a buffer comes back.
    pub(crate) fn begin_lines(&mut self) {
        if self.buffer.take().is_some() {
            self.rows.clear();
            self.pieces.clear();
        }
    }

    /// Drops cached rows outside `keep` once there are many more of them
    /// than a screen, and the kept pieces with them.
    pub(crate) fn evict(&mut self, keep: Range<usize>) {
        if self.rows.len() > 2 * keep.len() + 64 {
            self.rows.retain(|row, _| keep.contains(row));
            self.pieces.clear();
        }
    }

    /// Rows shaped so far.
    pub(crate) fn shaped_count(&self) -> u64 {
        self.shaped
    }

    /// The shaped `columns` of `row`, shaping them if the cache has none
    /// for the text there now. A `preedit` goes in at its column.
    pub(crate) fn row(
        &mut self,
        display: &DisplaySnapshot,
        row: usize,
        columns: Range<usize>,
        preedit: Option<(usize, &Preedit)>,
        system: &mut dyn TextSystem,
    ) -> &ShapedRow {
        let revision = self.revision;
        let unchanged = preedit.is_none()
            && self.rows.get(&row).is_some_and(|shaped| {
                shaped.revision == revision && shaped.columns == columns && !shaped.spliced
            });
        if unchanged {
            return &self.rows[&row];
        }
        self.read(display, row, &columns, preedit);
        let shaped = self.keep(row, columns, system, false);
        shaped.spliced = preedit.is_some();
        shaped
    }

    /// The shaped columns of `spans` up to `end`, as row `row`, shaping them if
    /// the cache has none for that text.
    pub(crate) fn line(
        &mut self,
        row: usize,
        spans: &[Span<'_>],
        end: usize,
        system: &mut dyn TextSystem,
    ) -> &ShapedRow {
        self.read_line(spans, end);
        self.keep(row, 0..end, system, true)
    }

    /// Makes what was just read the shaped `row`, shaping it unless the cache
    /// already has the same input. A short line of one piece, such as a line number, is a
    /// piece kept by its text too, so a scroll that moves numbers to other rows shapes only
    /// the new one, and a number that changes face shapes nothing (ADR 0105).
    fn keep(
        &mut self,
        row: usize,
        columns: Range<usize>,
        system: &mut dyn TextSystem,
        line: bool,
    ) -> &mut ShapedRow {
        let kept_whole = line && self.text.len() <= KEPT_PIECE_BYTES;
        if self.breaks.len() == 1 && !kept_whole {
            self.breaks.clear();
        }
        let revision = self.revision;
        let shaped = self.rows.entry(row).or_default();
        if shaped.text != self.text
            || shaped.graphemes != self.graphemes
            || shaped.breaks != self.breaks
        {
            self.shaped += 1;
            shape_pieces(
                system,
                &self.text,
                &self.breaks,
                &mut self.pieces,
                &mut self.glyphs,
            );
            self.placer.place(
                &self.graphemes,
                &self.glyphs,
                system.metrics().width,
                &mut shaped.glyphs,
            );
            shaped.text.clone_from(&self.text);
            shaped.graphemes.clone_from(&self.graphemes);
            shaped.breaks.clone_from(&self.breaks);
        }
        shaped.boxes.clone_from(&self.boxes);
        shaped.columns = columns;
        shaped.revision = revision;
        shaped
    }

    /// Reads the clusters of `row` that start in `columns`, and no further,
    /// into the scratch space: what to shape, and the boxes to draw. A `preedit`
    /// goes in at its column, and what follows it moves right.
    fn read(
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
        row_clusters(display, start, preedit, self.whitespace, |cluster, own| {
            if cluster.column >= columns.end {
                return ControlFlow::Break(());
            }
            let first = self.graphemes.len();
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
    fn read_line(&mut self, spans: &[Span<'_>], end: usize) {
        self.clear_scratch();
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

    fn clear_scratch(&mut self) {
        self.text.clear();
        self.graphemes.clear();
        self.breaks.clear();
        self.boxes.clear();
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
        cluster.for_each_run(|run| {
            let mut face = face.of_run(run.style);
            let mut text = run.text;
            if !whole && first_char(run.text).len() < run.text.len() {
                text = first_char(run.text);
                face = Face::Boxed;
                cut = true;
                // Cut clusters side by side share a box.
                match self.boxes.last_mut() {
                    Some((column, cells)) if self.cut_end == Some(run.column) => {
                        *cells = run.column + run.cells - *column;
                    }
                    _ => self.boxes.push((run.column, run.cells)),
                }
                self.cut_end = Some(run.column + run.cells);
            }
            // One box per boxed character, across its label's runs.
            if run.style == CellStyle::Boxed {
                if run.column == cluster.column {
                    self.boxes.push((run.column, run.cells));
                    self.cut_end = None;
                } else if let Some((_, cells)) = self.boxes.last_mut() {
                    *cells += run.cells;
                }
            }
            self.graphemes.push(Grapheme {
                byte: self.text.len() as u32,
                column: run.column,
                cells: u32::try_from(run.cells).unwrap_or(u32::MAX),
                face,
            });
            *spent += text.chars().count();
            self.text.push_str(text);
        });
        cut
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use oso_core::buffer::Buffer;
    use oso_core::display_map::{DisplayMap, TabWidth};
    use oso_core::rope::Rope;
    use oso_core::sum_tree::Bias;
    use oso_platform::text::{CellMetrics, FontId, GlyphBitmap, GlyphId, GlyphKey};
    use proptest::strategy::Strategy as _;

    /// A text system with one glyph per character, one cell wide, that
    /// records what it shaped. The glyph is the character, so stale text
    /// shows in the glyphs.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub(crate) shaped: Vec<String>,
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
            for (i, (byte, c)) in text.char_indices().enumerate() {
                out.push(ShapedGlyph {
                    key: GlyphKey {
                        font: FontId(0),
                        glyph: GlyphId(c as u16),
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
        let shaped = rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        assert_eq!(system.shaped, ["a", "U+202E", "b", "^M^G\\xFF", " c"]);
        assert_eq!(shaped.boxes, [(1, 6)]);
        let faces: Vec<_> = shaped.glyphs.iter().map(|g| (g.column, g.face)).collect();
        assert_eq!(faces[0], (0, Face::Text));
        assert_eq!(faces[1], (1, Face::Boxed));
        assert_eq!(faces[7], (7, Face::Text));
        assert_eq!(faces[8], (8, Face::Escape));
        assert_eq!(faces[12], (12, Face::Escape), "the invalid byte");
        assert_eq!(faces[16], (16, Face::Text), "the tab");
        assert_eq!(faces[17], (24, Face::Text), "after the tab stop");
    }

    #[test]
    fn each_invisible_character_gets_its_own_box() {
        let buffer = Buffer::from_text("t", "\u{202e}\u{202e}a\u{200b}");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        assert_eq!(shaped.boxes, [(0, 6), (6, 6), (13, 6)]);
    }

    #[test]
    fn rows_are_cached_by_display_row_and_reshaped_when_their_text_changes() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let buffer = Buffer::from_text("t", "one\ntwo\nthree");
        let display = buffer.display();
        let id = oso_core::editor::Editor::default().window().buffer();
        rows.begin(id, 1);
        rows.row(&display, 0, 0..128, None, &mut system);
        rows.row(&display, 1, 0..128, None, &mut system);
        rows.row(&display, 0, 0..128, None, &mut system);
        assert_eq!(rows.shaped_count(), 2, "row 0 came from the cache");
        rows.row(&display, 2, 0..128, None, &mut system);
        // More text on row 2, as while loading, reshapes only row 2.
        rows.begin(id, 2);
        let longer = Buffer::from_text("t", "one\ntwo\nthree four");
        rows.row(&longer.display(), 2, 0..128, None, &mut system);
        rows.row(&longer.display(), 1, 0..128, None, &mut system);
        assert_eq!(rows.shaped_count(), 4);
        assert_eq!(system.shaped.last().unwrap(), "three four");
        // Other columns shape again.
        rows.row(&longer.display(), 1, 64..192, None, &mut system);
        assert_eq!(rows.shaped_count(), 5);
    }

    #[test]
    fn marks_past_a_clusters_limit_are_shaped_as_labels_in_boxes() {
        let marks = format!("e{}", "\u{301}".repeat(100_000));
        let text = format!("{marks}\n{marks}");
        let buffer = Buffer::from_text("t", &text);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let row = rows.row(&buffer.display(), 0, 0..253, None, &mut system);
        assert_eq!(row.boxes.first(), Some(&(1, 6)));
        assert_eq!(row.boxes.len(), 42);
        let stack = format!("e{}", "\u{301}".repeat(8));
        assert_eq!(system.shaped[0], stack);
        assert_eq!(system.shaped[1], "U+0301".repeat(42));
    }

    /// `cluster` repeated, shaped in two windows of a row: all of its text
    /// reaches the shaper.
    fn assert_shaped_whole(cluster: &str, cells: usize) {
        let buffer = Buffer::from_text("t", &cluster.repeat(512));
        for columns in [0..128, 64..192, 128..256] {
            let mut rows = Rows::default();
            let mut system = Recorder::default();
            rows.row(&buffer.display(), 0, columns.clone(), None, &mut system);
            let count = columns.len() / cells;
            assert_eq!(system.shaped[0], cluster.repeat(count), "{columns:?}");
        }
    }

    #[test]
    fn decomposed_text_keeps_its_marks_across_a_row() {
        // U+1EAD as a base and two marks: three characters in one cell.
        assert_shaped_whole("a\u{323}\u{302}", 1);
    }

    #[test]
    fn emoji_sequences_stay_whole_across_a_row() {
        assert_shaped_whole("👨\u{200d}👩\u{200d}👧", 2);
    }

    #[test]
    fn the_budget_bounds_the_characters_of_a_row() {
        // Clusters at the display model's limits: on marks, with the longest
        // characters, and on characters.
        for cluster in [
            format!("e{}", "\u{1d165}".repeat(8)),
            "\u{600}".repeat(15) + "a",
        ] {
            let buffer = Buffer::from_text("t", &cluster.repeat(512));
            let mut rows = Rows::default();
            let mut system = Recorder::default();
            rows.row(&buffer.display(), 0, 0..128, None, &mut system);
            let shaped = &system.shaped[0];
            assert!(
                shaped.starts_with(&cluster),
                "the display model keeps it whole"
            );
            let bound = 128 * (CHARS_PER_COLUMN + 1) + cluster.chars().count();
            let chars = shaped.chars().count();
            assert!(chars <= bound, "{chars}");
            assert!(shaped.len() <= bound * 4, "{} bytes", shaped.len());
            assert!(
                chars >= 128 * CHARS_PER_COLUMN,
                "the budget is spent before clusters are cut: {chars}"
            );
        }
    }

    #[test]
    fn a_cluster_cut_past_the_budget_shows_in_a_box() {
        // 57 cells of 9 characters spend the budget of 128 columns, so the
        // clusters after them are cut.
        let spend = format!("e{}", "\u{301}".repeat(8)).repeat(57);
        let buffer = Buffer::from_text("t", &format!("{spend}e\u{301}xb\u{5bc}\u{5b8}"));
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        assert_eq!(system.shaped[0], spend, "shaped whole so far");
        assert_eq!(system.shaped[1..], ["e", "x", "b"]);
        assert_eq!(
            shaped.boxes,
            [(57, 1), (59, 1)],
            "only what lost characters"
        );
        let faces: Vec<_> = shaped.glyphs[shaped.glyphs.len() - 3..]
            .iter()
            .map(|g| (g.column, g.face))
            .collect();
        assert_eq!(
            faces,
            [(57, Face::Boxed), (58, Face::Text), (59, Face::Boxed)]
        );
    }

    #[test]
    fn each_cut_character_is_shaped_alone_and_cuts_side_by_side_share_a_box() {
        let spend = format!("e{}", "\u{301}".repeat(8)).repeat(57);
        let flag = "\u{1f1ef}\u{1f1f5}";
        let text = format!("{spend}\u{200b}\u{301}\u{302}{flag}{flag}a");
        let buffer = Buffer::from_text("t", &text);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        assert_eq!(
            system.shaped[1..],
            ["U+200B", "\u{301}", "\u{1f1ef}", "a"],
            "the second flag's piece is the first's, shaped once"
        );
        assert_eq!(shaped.boxes, [(57, 6), (63, 5)]);
        let columns: Vec<_> = shaped.glyphs[shaped.glyphs.len() - 4..]
            .iter()
            .map(|g| (g.column, g.face))
            .collect();
        assert_eq!(
            columns,
            [
                (63, Face::Boxed),
                (64, Face::Boxed),
                (66, Face::Boxed),
                (68, Face::Text)
            ]
        );
    }

    #[test]
    fn a_short_piece_is_shaped_once() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let spans = [span("name", Face::Text), span("12:5", Face::Text)];
        rows.line(0, &spans, 128, &mut system);
        let spans = [span("name", Face::Text), span("13:5", Face::Text)];
        rows.line(0, &spans, 128, &mut system);
        assert_eq!(system.shaped, ["name", "12:5", "13:5"]);
        rows.invalidate();
        rows.line(0, &spans, 128, &mut system);
        assert_eq!(
            system.shaped[3..],
            ["name", "13:5"],
            "a new font shapes again"
        );
    }

    #[test]
    fn line_numbers_that_move_rows_or_faces_are_not_shaped_again() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let numbers = |rows: &mut Rows, system: &mut Recorder, first: usize, current: usize| {
            for row in 0..3 {
                let text = format!("{:>3} ", first + row);
                let face = if first + row == current {
                    Face::LineNumberCurrent
                } else {
                    Face::LineNumber
                };
                rows.line(row, &[span(&text, face)], 128, system);
            }
        };
        numbers(&mut rows, &mut system, 1, 1);
        assert_eq!(system.shaped, ["  1 ", "  2 ", "  3 "]);
        numbers(&mut rows, &mut system, 2, 1);
        assert_eq!(
            system.shaped[3..],
            ["  4 "],
            "a scroll shapes the new number only"
        );
        numbers(&mut rows, &mut system, 2, 3);
        assert_eq!(
            system.shaped.len(),
            4,
            "another current line shapes nothing"
        );
    }

    #[test]
    fn another_buffers_pieces_are_dropped_with_its_rows() {
        let buffer = Buffer::from_text("t", "a\rb");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let mut editor = oso_core::editor::Editor::default();
        let first = editor.window().buffer();
        let second = editor.add_buffer(Buffer::from_text("t", ""));
        rows.begin(first, 1);
        rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        rows.begin(second, 1);
        rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        assert_eq!(system.shaped, ["a", "^M", "b", "a", "^M", "b"]);
    }

    #[test]
    fn kept_pieces_go_at_a_new_revision_and_with_evicted_rows() {
        let buffer = Buffer::from_text("t", &["a\rb"; 72].join("\n"));
        let display = buffer.display();
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.begin(buffer_id(), 1);
        for row in 0..70 {
            rows.row(&display, row, 0..128, None, &mut system);
        }
        assert_eq!(system.shaped, ["a", "^M", "b"], "shaped once for every row");
        rows.begin(buffer_id(), 2);
        rows.row(&display, 70, 0..128, None, &mut system);
        assert_eq!(system.shaped.len(), 6, "a new revision shapes them again");
        rows.evict(0..1);
        rows.row(&display, 71, 0..128, None, &mut system);
        assert_eq!(system.shaped.len(), 9, "evicted rows take their pieces");
    }

    #[test]
    fn a_long_line_shapes_only_the_columns_asked_for() {
        let line = "あ".repeat(1_000_000);
        let buffer = Buffer::from_text("t", &line);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 640..768, None, &mut system);
        assert_eq!(system.shaped[0].chars().count(), 64);
        assert_eq!(shaped.glyphs[0].column, 640);
    }

    #[test]
    fn a_preedit_is_shaped_apart_and_pushes_the_rest_of_its_row() {
        let buffer = Buffer::from_text("t", "abcd");
        let preedit = Preedit::new("かな".into(), None);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.begin(buffer_id(), 1);
        let shaped = rows.row(
            &buffer.display(),
            0,
            0..128,
            Some((2, &preedit)),
            &mut system,
        );
        assert_eq!(
            shown(shaped),
            [('a', 0), ('b', 1), ('か', 2), ('な', 4), ('c', 6), ('d', 7)]
        );
        assert_eq!(system.shaped, ["ab", "かな", "cd"]);
    }

    #[test]
    fn a_row_with_a_preedit_is_read_again_at_the_same_revision() {
        let buffer = Buffer::from_text("t", "abcd");
        let display = buffer.display();
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.begin(buffer_id(), 1);
        rows.row(&display, 0, 0..128, None, &mut system);
        let first = Preedit::new("か".into(), None);
        let shaped = rows.row(&display, 0, 0..128, Some((0, &first)), &mut system);
        assert_eq!(shown(shaped)[0], ('か', 0));
        let second = Preedit::new("き".into(), None);
        let shaped = rows.row(&display, 0, 0..128, Some((0, &second)), &mut system);
        assert_eq!(shown(shaped)[0], ('き', 0));
        // Gone, at the revision the row was first read at: the text is back.
        let shaped = rows.row(&display, 0, 0..128, None, &mut system);
        assert_eq!(shown(shaped)[0], ('a', 0));
        let count = rows.shaped_count();
        rows.row(&display, 0, 0..128, None, &mut system);
        assert_eq!(rows.shaped_count(), count, "then the revision holds again");
    }

    #[test]
    fn a_changing_preedit_shapes_only_its_own_row_and_that_row_once_after() {
        let buffer = Buffer::from_text("t", &"row of text\n".repeat(40));
        let display = buffer.display();
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.begin(buffer_id(), 1);
        let mut frame = |rows: &mut Rows, preedit: Option<&Preedit>| {
            let before = rows.shaped_count();
            for row in 0..40 {
                let spliced = preedit.filter(|_| row == 5).map(|preedit| (4, preedit));
                rows.row(&display, row, 0..128, spliced, &mut system);
            }
            rows.shaped_count() - before
        };
        frame(&mut rows, None);
        for text in ["か", "かな", "仮名"] {
            let preedit = Preedit::new(text.into(), None);
            assert_eq!(frame(&mut rows, Some(&preedit)), 1, "{text}");
        }
        assert_eq!(frame(&mut rows, None), 1, "the row goes back to its text");
        assert_eq!(frame(&mut rows, None), 0);
    }

    #[test]
    fn a_preedit_deep_in_a_long_line_reads_only_the_columns_asked_for() {
        let buffer = Buffer::from_text("t", &"あ".repeat(1_000_000));
        let preedit = Preedit::new("か".repeat(100), None);
        let mut rows = Rows::default();
        rows.begin(buffer_id(), 1);
        for columns in [
            999_936..1_000_064,
            1_000_064..1_000_192,
            1_000_192..1_000_320,
        ] {
            let mut system = Recorder::default();
            let start = columns.start;
            let spliced = Some((1_000_000, &preedit));
            let shaped = rows.row(&buffer.display(), 0, columns, spliced, &mut system);
            let read: usize = system.shaped.iter().map(|text| text.chars().count()).sum();
            assert_eq!(read, 64, "from {start}");
            assert_eq!(shaped.glyphs[0].column, start);
        }
    }

    #[test]
    fn a_preedit_takes_the_preedit_face_and_its_escapes_their_own() {
        let buffer = Buffer::from_text("t", "x");
        let preedit = Preedit::new("a\u{1}".into(), None);
        let mut rows = Rows::default();
        rows.begin(buffer_id(), 1);
        let mut system = Recorder::default();
        rows.row(
            &buffer.display(),
            0,
            0..128,
            Some((0, &preedit)),
            &mut system,
        );
        let faces: Vec<_> = rows.graphemes.iter().map(|g| g.face).collect();
        assert_eq!(
            faces,
            [Face::Preedit, Face::Escape, Face::Escape, Face::Text]
        );
    }

    /// A buffer's text and display model, edited as a buffer does.
    struct Text {
        rope: Rope,
        map: DisplayMap,
    }

    impl Text {
        fn new(text: &str) -> Self {
            Self::with_tab_width(text, TabWidth::default())
        }

        fn with_tab_width(text: &str, tab_width: TabWidth) -> Self {
            let rope = Rope::from(text);
            let map = DisplayMap::new(&rope, tab_width);
            Self { rope, map }
        }

        fn replace(&mut self, range: Range<usize>, text: &str) {
            let edit = self.rope.replace(range, text);
            self.map.sync(&self.rope, &edit);
        }
    }

    /// A cache with a recorder, whose frames each have a new revision.
    struct Frames {
        rows: Rows,
        system: Recorder,
        revision: u64,
    }

    impl Frames {
        fn new() -> Self {
            Self {
                rows: Rows::default(),
                system: Recorder::default(),
                revision: 0,
            }
        }

        /// Draws `rows` of `text` after a change, and returns how many of
        /// them were shaped.
        fn draw(&mut self, text: &Text, rows: Range<usize>, columns: Range<usize>) -> u64 {
            self.draw_display(&text.map.snapshot(), rows, columns)
        }

        fn draw_display(
            &mut self,
            display: &DisplaySnapshot,
            rows: Range<usize>,
            columns: Range<usize>,
        ) -> u64 {
            self.revision += 1;
            self.rows.begin(buffer_id(), self.revision);
            let before = self.rows.shaped_count();
            for row in rows {
                self.rows
                    .row(display, row, columns.clone(), None, &mut self.system);
            }
            self.rows.shaped_count() - before
        }

        /// What the cached `row` shows, as characters and their columns.
        fn shown(&self, row: usize) -> Vec<(char, usize)> {
            shown(&self.rows.rows[&row])
        }
    }

    fn buffer_id() -> BufferId {
        oso_core::editor::Editor::default().window().buffer()
    }

    fn shown(row: &ShapedRow) -> Vec<(char, usize)> {
        row.glyphs
            .iter()
            .map(|g| (char::from_u32(u32::from(g.key.glyph.0)).unwrap(), g.column))
            .collect()
    }

    fn columns_from(start: usize, text: &str) -> Vec<(char, usize)> {
        text.chars().zip(start..).collect()
    }

    #[test]
    fn a_replacement_of_the_same_length_shows_the_new_text() {
        let mut text = Text::new("one\ntwo\nsix");
        let mut frames = Frames::new();
        assert_eq!(frames.draw(&text, 0..3, 0..128), 3);
        text.replace(4..7, "TWO");
        assert_eq!(frames.draw(&text, 0..3, 0..128), 1, "only the edited row");
        assert_eq!(frames.shown(1), columns_from(0, "TWO"));
        assert_eq!(frames.system.shaped.last().unwrap(), "TWO");
    }

    #[test]
    fn an_insertion_reshapes_only_its_own_row() {
        let mut text = Text::new("one\ntwo\nsix\nten");
        let mut frames = Frames::new();
        frames.draw(&text, 0..4, 0..128);
        text.replace(5..5, "XX");
        assert_eq!(frames.draw(&text, 0..4, 0..128), 1, "rows below moved");
        assert_eq!(frames.system.shaped.last().unwrap(), "tXXwo");
        // Nothing in a reused row depends on where it is in the buffer.
        assert_eq!(frames.shown(2), columns_from(0, "six"));
        assert_eq!(frames.shown(3), columns_from(0, "ten"));
    }

    #[test]
    fn a_line_break_moves_rows_by_shaping_them_again() {
        let mut text = Text::new("one\ntwo\nsix\nten");
        let mut frames = Frames::new();
        frames.draw(&text, 0..4, 0..128);
        text.replace(4..4, "\n");
        // Rows are cached by display row, so the rows below show other text
        // there now, and the row above is left alone.
        assert_eq!(frames.draw(&text, 0..5, 0..128), 4);
        assert_eq!(frames.shown(0), columns_from(0, "one"));
        assert!(frames.shown(1).is_empty());
        assert_eq!(frames.shown(2), columns_from(0, "two"));
        assert_eq!(frames.shown(3), columns_from(0, "six"));
        assert_eq!(frames.shown(4), columns_from(0, "ten"));
    }

    #[test]
    fn a_deleted_line_moves_rows_up() {
        let mut text = Text::new("one\ntwo\nsix");
        let mut frames = Frames::new();
        frames.draw(&text, 0..3, 0..128);
        text.replace(0..4, "");
        assert_eq!(frames.draw(&text, 0..2, 0..128), 2);
        assert_eq!(frames.shown(0), columns_from(0, "two"));
        assert_eq!(frames.shown(1), columns_from(0, "six"));
    }

    #[test]
    fn a_new_revision_with_no_change_shapes_nothing() {
        let text = Text::new("one\ntwo\n\tsix\u{202e}");
        let mut frames = Frames::new();
        frames.draw(&text, 0..3, 0..128);
        let glyphs = frames.rows.rows[&2].glyphs.clone();
        let boxes = frames.rows.rows[&2].boxes.clone();
        assert_eq!(frames.draw(&text, 0..3, 0..128), 0);
        assert_eq!(frames.rows.rows[&2].glyphs, glyphs);
        assert_eq!(frames.rows.rows[&2].boxes, boxes);
    }

    #[test]
    fn the_same_revision_does_not_look_at_the_buffer() {
        let mut text = Text::new("one");
        let mut frames = Frames::new();
        frames.draw(&text, 0..1, 0..128);
        // A text the revision does not know about: never read.
        text.replace(0..3, "TWO");
        frames
            .rows
            .row(&text.map.snapshot(), 0, 0..128, None, &mut frames.system);
        assert_eq!(frames.shown(0), columns_from(0, "one"));
    }

    #[test]
    fn a_row_changed_back_to_its_old_text_shows_it_again() {
        let mut text = Text::new("one\ntwo\nsix");
        let mut frames = Frames::new();
        frames.draw(&text, 0..3, 0..128);
        text.replace(4..7, "TWO");
        assert_eq!(frames.draw(&text, 0..3, 0..128), 1);
        text.replace(4..7, "two");
        assert_eq!(frames.draw(&text, 0..3, 0..128), 1);
        assert_eq!(frames.shown(1), columns_from(0, "two"));
        // An undo with no frame in between leaves nothing to do.
        text.replace(4..7, "TWO");
        text.replace(4..7, "two");
        assert_eq!(frames.draw(&text, 0..3, 0..128), 0);
    }

    #[test]
    fn a_long_line_compares_only_the_columns_shaped() {
        let mut text = Text::new(&"a".repeat(10_000_000));
        let mut frames = Frames::new();
        frames.draw(&text, 0..1, 640..768);
        // Past the columns, and before them with the same width.
        text.replace(5_000_000..5_000_001, "b");
        text.replace(10..11, "b");
        assert_eq!(frames.draw(&text, 0..1, 640..768), 0);
        assert_eq!(frames.rows.text.len(), 128, "stopped at the last column");
        text.replace(700..701, "b");
        assert_eq!(frames.draw(&text, 0..1, 640..768), 1);
        assert_eq!(frames.shown(0)[60], ('b', 700));
        // The same text in other columns.
        assert_eq!(frames.draw(&text, 0..1, 704..832), 1);
    }

    #[test]
    fn a_wider_window_keeps_a_row_that_ends_inside_both() {
        let text = Text::new("one");
        let mut frames = Frames::new();
        frames.draw(&text, 0..1, 0..128);
        assert_eq!(frames.draw(&text, 0..1, 0..192), 0);
        assert_eq!(frames.shown(0), columns_from(0, "one"));
    }

    #[test]
    fn a_wide_cluster_at_the_start_column_keeps_its_column() {
        let mut text = Text::new(&format!("{}あbc", "a".repeat(63)));
        let mut frames = Frames::new();
        frames.draw(&text, 0..1, 64..192);
        assert_eq!(frames.shown(0), [('あ', 63), ('b', 65), ('c', 66)]);
        // The bytes from the start column on are the same, one cell later.
        text.replace(0..0, "a");
        assert_eq!(frames.draw(&text, 0..1, 64..192), 1);
        assert_eq!(frames.shown(0), [('あ', 64), ('b', 66), ('c', 67)]);
    }

    #[test]
    fn a_tab_at_the_start_column_keeps_its_column() {
        let tab_width = TabWidth::new(3);
        let mut text = Text::with_tab_width(&format!("{}\tx", "a".repeat(63)), tab_width);
        let mut frames = Frames::new();
        frames.draw(&text, 0..1, 64..192);
        assert_eq!(frames.shown(0), [(' ', 63), ('x', 66)]);
        // The tab now starts a cell later and ends at the same stop, so
        // only its first cell tells the two rows apart.
        text.replace(0..0, "a");
        assert_eq!(frames.draw(&text, 0..1, 64..192), 1);
        assert_eq!(frames.shown(0), [(' ', 64), ('x', 66)]);
    }

    #[test]
    fn a_tab_width_change_moves_the_text_after_a_tab() {
        let rope = Rope::from("a\tb");
        let mut frames = Frames::new();
        let mut map = DisplayMap::new(&rope, TabWidth::new(8));
        let text = Text {
            rope: rope.clone(),
            map: map.clone(),
        };
        frames.draw(&text, 0..1, 0..128);
        assert_eq!(frames.shown(0), [('a', 0), (' ', 1), ('b', 8)]);
        map.set_tab_width(&rope, TabWidth::new(4));
        let text = Text { rope, map };
        assert_eq!(frames.draw(&text, 0..1, 0..128), 1);
        assert_eq!(frames.shown(0), [('a', 0), (' ', 1), ('b', 4)]);
    }

    #[test]
    fn invalid_bytes_with_the_same_text_in_the_rope_are_told_apart() {
        let (one, other) = (
            Buffer::from_bytes("t", b"a\xfe"),
            Buffer::from_bytes("t", b"a\xff"),
        );
        let replacement = Buffer::from_text("t", "a\u{fffd}");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let id = buffer_id();
        for (revision, (buffer, label)) in [
            (&one, "\\xFE"),
            (&other, "\\xFF"),
            (&replacement, "a\u{fffd}"),
        ]
        .into_iter()
        .enumerate()
        {
            rows.begin(id, revision as u64);
            rows.row(&buffer.display(), 0, 0..128, None, &mut system);
            assert_eq!(system.shaped.last().unwrap(), label);
        }
        assert_eq!(rows.shaped_count(), 3);
    }

    #[test]
    fn text_that_looks_like_an_escape_is_told_apart_from_one() {
        let (escape, typed) = (
            Buffer::from_bytes("t", b"\xff"),
            Buffer::from_text("t", "\\xFF"),
        );
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let id = buffer_id();
        rows.begin(id, 1);
        let faces = |row: &ShapedRow| row.glyphs.iter().map(|g| g.face).collect::<Vec<_>>();
        let first = faces(rows.row(&escape.display(), 0, 0..128, None, &mut system));
        rows.begin(id, 2);
        let second = faces(rows.row(&typed.display(), 0, 0..128, None, &mut system));
        assert_eq!(first, [Face::Escape; 4]);
        assert_eq!(second, [Face::Text; 4]);
    }

    fn span(text: &str, face: Face) -> Span<'_> {
        Span { text, face }
    }

    #[test]
    fn spans_add_up_their_columns_and_keep_their_faces() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let spans = [span("aあ", Face::Cursor), span("bc", Face::Text)];
        let shaped = rows.line(0, &spans, 128, &mut system);
        assert_eq!(shown(shaped), [('a', 0), ('あ', 1), ('b', 3), ('c', 4)]);
        let faces: Vec<_> = shaped.glyphs.iter().map(|g| g.face).collect();
        assert_eq!(faces, [Face::Cursor, Face::Cursor, Face::Text, Face::Text]);
    }

    #[test]
    fn a_line_shows_controls_as_escapes_and_never_shapes_them() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let line = [span("a\u{202e}b\r", Face::Cursor)];
        let shaped = rows.line(0, &line, 128, &mut system);
        assert_eq!(system.shaped, ["a", "U+202E", "b", "^M"]);
        assert_eq!(shaped.boxes, [(1, 6)]);
        assert_eq!(shaped.glyphs[0].face, Face::Cursor);
        assert_eq!(shaped.glyphs[1].face, Face::Boxed);
        assert_eq!(shaped.glyphs[8].face, Face::Escape);
    }

    #[test]
    fn line_breaks_and_tabs_in_a_span_do_not_end_the_row() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let spans = [span("a\nb\tc\r\n", Face::Cursor), span("warn", Face::Text)];
        let shaped = rows.line(0, &spans, 128, &mut system);
        assert_eq!(system.shaped, ["a", "^J", "b", "^I", "c", "^M^J", "warn"]);
        let last = shaped.glyphs.last().unwrap();
        assert_eq!((last.column, last.face), (14, Face::Text));
        assert_eq!(shaped.glyphs[1].face, Face::Escape);
    }

    /// A text system that joins what it is given, as a real shaper does: a mark
    /// goes on the character before it, and `->` is one glyph over two cells.
    struct Joiner;

    impl TextSystem for Joiner {
        fn metrics(&self) -> CellMetrics {
            Recorder::default().metrics()
        }

        fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
            out.clear();
            let mut x = 0.0;
            let mut base = None;
            for (byte, c) in text.char_indices() {
                if c == '>' && text[..byte].ends_with('-') {
                    continue;
                }
                let mark = matches!(u32::from(c), 0x300..=0x36f);
                let cluster = if mark { base.unwrap_or(byte) } else { byte };
                let advance = if text[byte..].starts_with("->") {
                    20.0
                } else if mark && base.is_some() {
                    0.0
                } else {
                    10.0
                };
                out.push(ShapedGlyph {
                    key: GlyphKey {
                        font: FontId(0),
                        glyph: GlyphId(c as u16),
                    },
                    cluster: cluster as u32,
                    x,
                    y: 0.0,
                    advance,
                });
                x += advance;
                base = Some(cluster);
            }
        }

        fn rasterize(&mut self, _key: GlyphKey) -> Option<GlyphBitmap> {
            None
        }
    }

    fn joined(spans: &[Span<'_>]) -> Vec<(char, usize, u32)> {
        let mut rows = Rows::default();
        rows.line(0, spans, 128, &mut Joiner)
            .glyphs
            .iter()
            .map(|g| {
                let c = char::from_u32(u32::from(g.key.glyph.0)).unwrap();
                (c, g.column, g.cells)
            })
            .collect()
    }

    #[test]
    fn the_fake_joins_what_is_in_one_span() {
        let glyphs = joined(&[span("a\u{301}b", Face::Text)]);
        assert_eq!(glyphs, [('a', 0, 1), ('\u{301}', 0, 1), ('b', 1, 1)]);
        assert_eq!(joined(&[span("->", Face::Text)]), [('-', 0, 2)]);
    }

    #[test]
    fn a_mark_at_the_start_of_a_span_stays_off_the_span_before_it() {
        let glyphs = joined(&[span("a", Face::Cursor), span("\u{301}b", Face::Text)]);
        assert_eq!(glyphs, [('a', 0, 1), ('\u{301}', 1, 1), ('b', 2, 1)]);
        // The same in one face: where one span ends is what counts.
        let glyphs = joined(&[span("a", Face::Text), span("\u{301}b", Face::Text)]);
        assert_eq!(glyphs, [('a', 0, 1), ('\u{301}', 1, 1), ('b', 2, 1)]);
    }

    #[test]
    fn a_ligature_does_not_form_across_spans_or_faces() {
        let apart = [('-', 0, 1), ('>', 1, 1)];
        assert_eq!(
            joined(&[span("-", Face::Text), span(">", Face::Text)]),
            apart
        );
        assert_eq!(
            joined(&[span("-", Face::Text), span(">", Face::Cursor)]),
            apart
        );
    }

    #[test]
    fn a_label_is_shaped_apart_from_a_mark_after_it() {
        let expected = [('^', 0, 1), ('M', 1, 1), ('\u{301}', 2, 1)];
        assert_eq!(joined(&[span("\r\u{301}", Face::Text)])[..3], expected);
        let buffer = Buffer::from_text("t", "\r\u{301}");
        let row = Rows::default()
            .row(&buffer.display(), 0, 0..128, None, &mut Joiner)
            .glyphs
            .clone();
        let shown: Vec<_> = row
            .iter()
            .map(|g| {
                let c = char::from_u32(u32::from(g.key.glyph.0)).unwrap();
                (c, g.column, g.cells)
            })
            .collect();
        assert_eq!(shown[..3], expected);
    }

    #[test]
    fn a_later_span_keeps_its_budget_of_characters() {
        let long = ("\u{600}".repeat(15) + "a").repeat(25);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let spans = [span(&long, Face::Text), span("e\u{301}", Face::Cursor)];
        rows.line(0, &spans, 100, &mut system);
        assert_eq!(system.shaped.last().unwrap(), "e\u{301}");
    }

    #[test]
    fn the_same_text_split_otherwise_is_shaped_again() {
        let mut rows = Rows::default();
        let whole = [span("->", Face::Text)];
        assert_eq!(rows.line(0, &whole, 128, &mut Joiner).glyphs.len(), 1);
        let apart = [span("-", Face::Text), span(">", Face::Text)];
        assert_eq!(rows.line(0, &apart, 128, &mut Joiner).glyphs.len(), 2);
        assert_eq!(rows.shaped_count(), 2);
        assert_eq!(rows.line(0, &apart, 128, &mut Joiner).glyphs.len(), 2);
        assert_eq!(rows.shaped_count(), 2, "the same split is not shaped again");
    }

    #[test]
    fn a_row_of_one_face_is_shaped_whole() {
        let buffer = Buffer::from_text("t", "one row\t->");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        assert_eq!(system.shaped.len(), 1);
        assert!(rows.breaks.is_empty());
        let mut system = Recorder::default();
        rows.line(0, &[span("one row", Face::Text)], 128, &mut system);
        assert_eq!(system.shaped, ["one row"]);
        assert_eq!(
            rows.breaks,
            [0],
            "a short line is one piece kept by its text"
        );
        let long = "x".repeat(KEPT_PIECE_BYTES + 1);
        rows.line(0, &[span(&long, Face::Text)], 128, &mut system);
        assert!(rows.breaks.is_empty());
    }

    #[test]
    fn a_buffers_row_is_shaped_in_pieces_where_the_face_changes() {
        let buffer = Buffer::from_text("t", "a\rb");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        let columns: Vec<_> = shaped.glyphs.iter().map(|g| g.column).collect();
        assert_eq!(columns, [0, 1, 2, 3]);
        assert_eq!(system.shaped, ["a", "^M", "b"]);
    }

    #[test]
    fn a_line_stops_at_the_last_column() {
        let text = "a".repeat(10_000_000);
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.line(0, &[span(&text, Face::Text)], 128, &mut system);
        assert_eq!(system.shaped[0].len(), 128);
    }

    #[test]
    fn a_wide_cluster_in_the_last_column_is_read_whole() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.line(0, &[span("aあb", Face::Text)], 2, &mut system);
        assert_eq!(shown(shaped), [('a', 0), ('あ', 1)]);
    }

    #[test]
    fn a_span_is_cut_to_what_its_columns_can_show_on_a_character() {
        assert_eq!(visible_part("short", 1), "short");
        let marks = format!("e{}", "\u{301}".repeat(1000));
        let cut = visible_part(&marks, 2);
        assert_eq!(
            cut.len(),
            2 * MAX_CLUSTER_BYTES - 1,
            "1 byte for the base, and 2 for each mark"
        );
        assert_eq!(visible_part(&marks, usize::MAX), marks);
    }

    /// What `text` lays out in the columns before `end`.
    fn clusters_before(text: &str, end: usize) -> Vec<(usize, usize, String)> {
        text_clusters(text, 0)
            .take_while(|cluster| cluster.column < end)
            .map(|cluster| (cluster.column, cluster.cells, cluster.text.to_owned()))
            .collect()
    }

    #[test]
    fn cutting_a_span_keeps_a_cluster_of_the_most_bytes_in_one_column() {
        // A character that joins what follows it, 4 bytes long and 1 cell wide.
        let text = "\u{110bd}".repeat(40);
        let first = text_clusters(&text, 0).next().unwrap();
        assert_eq!(first.text.len(), MAX_CLUSTER_BYTES, "the limit is reached");
        assert_eq!(visible_part(&text, 1).len(), MAX_CLUSTER_BYTES);
        let (cut, whole) = (visible_part(&text, 1), text.as_str());
        assert_eq!(clusters_before(cut, 1), clusters_before(whole, 1));
    }

    #[test]
    fn a_line_with_the_same_text_is_not_shaped_again() {
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.begin_lines();
        rows.line(0, &[span("one", Face::Text)], 128, &mut system);
        rows.line(0, &[span("one", Face::Text)], 128, &mut system);
        assert_eq!(rows.shaped_count(), 1);
        // Another row, other text, and the same text in another face.
        rows.line(1, &[span("one", Face::Text)], 128, &mut system);
        rows.line(0, &[span("two", Face::Text)], 128, &mut system);
        assert_eq!(rows.shaped_count(), 3);
        let shaped = rows.line(0, &[span("two", Face::Cursor)], 128, &mut system);
        assert_eq!(shaped.glyphs[0].face, Face::Cursor);
    }

    #[test]
    fn a_buffer_coming_back_after_lines_is_read_again() {
        let buffer = Buffer::from_text("t", "text");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let id = buffer_id();
        rows.begin(id, 1);
        rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        rows.begin_lines();
        let line = rows.line(0, &[span("line", Face::Text)], 128, &mut system);
        assert_eq!(shown(line), columns_from(0, "line"));
        rows.begin(id, 1);
        let row = rows.row(&buffer.display(), 0, 0..128, None, &mut system);
        assert_eq!(shown(row), columns_from(0, "text"));
    }

    #[test]
    fn boxes_follow_the_text() {
        let mut text = Text::new("\u{202e}\u{200b}");
        let mut frames = Frames::new();
        frames.draw(&text, 0..1, 0..128);
        assert_eq!(frames.rows.rows[&0].boxes, [(0, 6), (6, 6)]);
        text.replace(3..6, "");
        frames.draw(&text, 0..1, 0..128);
        assert_eq!(frames.rows.rows[&0].boxes, [(0, 6)]);
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
                rows.row(&display, row, 0..256, None, &mut system);
            }
            for shaped in &system.shaped {
                for c in shaped.chars() {
                    let bidi = matches!(u32::from(c), 0x61c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069);
                    proptest::prop_assert!(!c.is_control() && !bidi, "{c:?} in {shaped:?}");
                }
            }
        }

        /// Whatever the text and however it is split, a row read again shows what a
        /// new cache shapes.
        #[test]
        fn prop_a_cached_line_shows_the_same_as_a_new_one(
            frames in proptest::collection::vec(
                (proptest::collection::vec(proptest::sample::select(LINE_PIECES), 0..12), 0usize..12, 1usize..32),
                1..8,
            ),
        ) {
            let mut cached = Rows::default();
            for (pieces, split, end) in frames {
                let split = split.min(pieces.len());
                let (first, second) = (pieces[..split].concat(), pieces[split..].concat());
                let spans = [span(&first, Face::Cursor), span(&second, Face::Text)];
                let old = cached.line(0, &spans, end, &mut Joiner).glyphs.clone();
                let mut fresh = Rows::default();
                let new = fresh.line(0, &spans, end, &mut Joiner).glyphs.clone();
                proptest::prop_assert_eq!(old, new);
            }
        }

        /// Cutting text to what its columns can show lays out those columns the
        /// same, for clusters at their limits too.
        #[test]
        fn prop_cutting_a_span_keeps_the_columns_it_can_show(
            pieces in proptest::collection::vec(0usize..10, 0..80),
            end in 1usize..16,
        ) {
            const PIECES: [&str; 10] = ["a", "あ", "\r\n", "\n", "\t", "\u{202e}", "\u{110bd}",
                                        "e\u{301}\u{301}", "\u{1f468}\u{200d}", "\u{1d165}"];
            let text: String = pieces.iter().map(|&i| PIECES[i]).collect();
            proptest::prop_assert_eq!(
                clusters_before(visible_part(&text, end), end),
                clusters_before(&text, end)
            );
        }

        /// The same for text that is no buffer's, however it is split into spans.
        #[test]
        fn prop_a_line_never_gives_the_shaper_controls(
            pieces in proptest::collection::vec(0usize..12, 0..60),
            splits in proptest::collection::vec(0usize..60, 0..6),
            end in 1usize..256,
        ) {
            const PIECES: [&str; 12] = ["a", "あ", "\r", "\n", "\r\n", "\t", "\u{202e}", "\u{2066}",
                                        "\u{200b}", "\u{7}", "\u{85}", "e\u{301}"];
            let mut splits = splits;
            splits.push(0);
            splits.push(pieces.len());
            splits.sort_unstable();
            let spans: Vec<String> = splits
                .windows(2)
                .map(|w| pieces[w[0].min(pieces.len())..w[1].min(pieces.len())].iter().map(|&i| PIECES[i]).collect())
                .collect();
            let spans: Vec<_> = spans.iter().map(|text| span(text, Face::Text)).collect();
            let mut rows = Rows::default();
            let mut system = Recorder::default();
            let shaped = rows.line(0, &spans, end, &mut system);
            let mut last = 0;
            for g in &shaped.glyphs {
                proptest::prop_assert!(g.column >= last, "columns are in order");
                last = g.column;
            }
            for c in system.shaped.iter().flat_map(|text| text.chars()) {
                let bidi = matches!(u32::from(c), 0x61c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069);
                proptest::prop_assert!(!c.is_control() && !bidi, "{:?}", c);
            }
        }

        /// After any edits, every cached row shows what a new cache shapes.
        #[test]
        fn prop_a_cached_row_shows_the_same_as_a_new_one(
            pad in proptest::bool::ANY,
            start in text_strategy(),
            edits in proptest::collection::vec((proptest::num::usize::ANY, 0usize..8, text_strategy()), 1..8),
            tab_width in 1u32..9,
            columns in proptest::sample::select(&[0usize, 64][..]),
        ) {
            // Padded, row 1 crosses from one rope chunk to the next.
            let start = if pad { format!("{}\n{start}", "a".repeat(950)) } else { start };
            let mut text = Text::with_tab_width(&start, TabWidth::new(tab_width));
            let mut frames = Frames::new();
            let columns = columns..columns + 128;
            let rows = |text: &Text| 0..text.map.snapshot().row_count().min(32);
            frames.draw(&text, rows(&text), columns.clone());
            for (at, len, inserted) in edits {
                let from = text.rope.clip_offset(at % (text.rope.len() + 1), Bias::Left);
                let to = text.rope.clip_offset((from + len).min(text.rope.len()), Bias::Right);
                text.replace(from..to, &inserted);
                frames.draw(&text, rows(&text), columns.clone());
                let display = text.map.snapshot();
                prop_same_as_new(&frames.rows, &display, rows(&text), &columns)?;
            }
        }

        /// The same for buffers whose bytes differ only in what the rope
        /// does not hold, such as the value of an invalid byte.
        #[test]
        fn prop_a_cached_row_shows_the_same_as_a_new_one_after_a_reload(
            files in proptest::collection::vec(proptest::collection::vec(0u8..8, 0..40), 1..6),
        ) {
            const BYTES: [&[u8]; 8] = [b"a", b"\xfe", b"\xff", b"\xef\xbf\xbd", b"\\xFF", b"\t", b" ", b"\n"];
            let mut frames = Frames::new();
            for file in files {
                let bytes: Vec<u8> = file.iter().flat_map(|&i| BYTES[usize::from(i)].iter().copied()).collect();
                let display = Buffer::from_bytes("t", &bytes).display();
                let rows = 0..display.row_count();
                frames.draw_display(&display, rows.clone(), 0..128);
                prop_same_as_new(&frames.rows, &display, rows, &(0..128))?;
            }
        }
    }

    /// Fails if a cached row differs from what a new cache shapes.
    fn prop_same_as_new(
        cached: &Rows,
        display: &DisplaySnapshot,
        rows: Range<usize>,
        columns: &Range<usize>,
    ) -> Result<(), proptest::test_runner::TestCaseError> {
        let mut fresh = Rows::default();
        let mut system = Recorder::default();
        for row in rows {
            let new = fresh.row(display, row, columns.clone(), None, &mut system);
            let old = &cached.rows[&row];
            proptest::prop_assert_eq!(&old.glyphs, &new.glyphs, "row {}", row);
            proptest::prop_assert_eq!(&old.boxes, &new.boxes, "row {}", row);
        }
        Ok(())
    }

    const LINE_PIECES: &[&str] = &[
        "a", "->", "-", ">", "あ", "\r\n", "\t", "\u{202e}", "\u{301}", "e\u{301}",
    ];

    /// Text with line breaks, tabs, wide and combining characters, and
    /// characters that are drawn as escapes.
    fn text_strategy() -> impl proptest::strategy::Strategy<Value = String> {
        const PIECES: &[&str] = &[
            "a",
            "bc",
            "\n",
            "\r\n",
            "\t",
            "あ",
            "e\u{301}",
            "\u{301}",
            "\u{202e}",
            "\u{7}",
            "👨\u{200d}👩",
            "\u{fffd}",
        ];
        proptest::collection::vec(proptest::sample::select(PIECES), 0..80)
            .prop_map(|pieces| pieces.concat())
    }
}
