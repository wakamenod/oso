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
//!   and invisible characters get a box.

use std::collections::HashMap;
use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::display_map::{CellStyle, DisplayPoint, DisplaySnapshot};
use oso_platform::text::{ShapedGlyph, TextSystem};

use crate::grid::{Grapheme, PlacedGlyph, Placer};

/// The most characters of one cluster passed to the shaper: enough for
/// the longest emoji sequences and Indic clusters. A cluster of a base and
/// thousands of combining marks takes one cell; shaping all of it on the
/// UI thread would let a small file stall a frame.
const MAX_CLUSTER_CHARS: usize = 16;

/// The characters shaped per row, per column shaped. Real text has at most
/// three or so; past this budget, clusters are shaped by their first
/// character only. A row's text then has at most this many characters per
/// column plus one more, and one cluster: hostile text costs a bounded
/// amount per row, about 20 bytes per column at worst.
const CHARS_PER_COLUMN: usize = 4;

/// `text` cut to its first `chars` characters.
fn clip_cluster(text: &str, chars: usize) -> &str {
    match text.char_indices().nth(chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
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
    /// What the shaper and the placer were given. The glyphs depend on
    /// nothing else but the font, so the same input gives the same glyphs,
    /// and comparing the bytes leaves no false match that could show stale
    /// text. Nothing here or in the glyphs is a buffer offset, so a row
    /// stays valid wherever its text moves in the buffer.
    text: String,
    graphemes: Vec<Grapheme>,
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
    /// Scratch space for reading a row.
    text: String,
    graphemes: Vec<Grapheme>,
    boxes: Vec<(usize, usize)>,
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
        self.read(display, row, &columns);
        let shaped = self.rows.entry(row).or_default();
        if shaped.text != self.text || shaped.graphemes != self.graphemes {
            self.shaped += 1;
            system.shape(&self.text, &mut self.glyphs);
            self.placer.place(
                &self.graphemes,
                &self.glyphs,
                system.metrics().width,
                &mut shaped.glyphs,
            );
            shaped.text.clone_from(&self.text);
            shaped.graphemes.clone_from(&self.graphemes);
        }
        shaped.boxes.clone_from(&self.boxes);
        shaped.columns = columns;
        shaped.revision = revision;
        shaped
    }

    /// Reads the clusters of `row` that start in `columns`, and no further,
    /// into the scratch space: what to shape, and the boxes to draw.
    fn read(&mut self, display: &DisplaySnapshot, row: usize, columns: &Range<usize>) {
        self.text.clear();
        self.graphemes.clear();
        self.boxes.clear();
        let budget = columns.len().saturating_mul(CHARS_PER_COLUMN);
        let mut spent = 0;
        for cluster in display.clusters(DisplayPoint::new(row, columns.start)) {
            if cluster.column >= columns.end {
                break;
            }
            let chars = if spent < budget { MAX_CLUSTER_CHARS } else { 1 };
            cluster.for_each_run(|run| {
                // One box per invisible character, across its label's runs.
                if run.style == CellStyle::Boxed {
                    if run.column == cluster.column {
                        self.boxes.push((run.column, run.cells));
                    } else if let Some((_, cells)) = self.boxes.last_mut() {
                        *cells += run.cells;
                    }
                }
                self.graphemes.push(Grapheme {
                    byte: self.text.len() as u32,
                    column: run.column,
                    cells: u32::try_from(run.cells).unwrap_or(u32::MAX),
                    style: run.style,
                });
                let text = clip_cluster(run.text, chars);
                spent += text.chars().count();
                self.text.push_str(text);
            });
        }
    }
}

#[cfg(test)]
mod tests {
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
    fn each_invisible_character_gets_its_own_box() {
        let buffer = Buffer::from_text("t", "\u{202e}\u{202e}a\u{200b}");
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        let shaped = rows.row(&buffer.display(), 0, 0..128, &mut system);
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
        rows.row(&display, 0, 0..128, &mut system);
        rows.row(&display, 1, 0..128, &mut system);
        rows.row(&display, 0, 0..128, &mut system);
        assert_eq!(rows.shaped_count(), 2, "row 0 came from the cache");
        rows.row(&display, 2, 0..128, &mut system);
        // More text on row 2, as while loading, reshapes only row 2.
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
            shaped <= 256 * (CHARS_PER_COLUMN + 1) + MAX_CLUSTER_CHARS,
            "{shaped} characters shaped"
        );
    }

    /// `cluster` repeated, shaped in two windows of a row: all of its text
    /// reaches the shaper.
    fn assert_shaped_whole(cluster: &str, cells: usize) {
        let buffer = Buffer::from_text("t", &cluster.repeat(512));
        for columns in [0..128, 64..192, 128..256] {
            let mut rows = Rows::default();
            let mut system = Recorder::default();
            rows.row(&buffer.display(), 0, columns.clone(), &mut system);
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
        // Clusters of the longest characters, each over the cluster limit.
        let cluster = format!("e{}", "\u{1d165}".repeat(MAX_CLUSTER_CHARS * 2));
        let buffer = Buffer::from_text("t", &cluster.repeat(512));
        let mut rows = Rows::default();
        let mut system = Recorder::default();
        rows.row(&buffer.display(), 0, 0..128, &mut system);
        let shaped = &system.shaped[0];
        let bound = 128 * (CHARS_PER_COLUMN + 1) + MAX_CLUSTER_CHARS;
        assert!(
            shaped.chars().count() <= bound,
            "{}",
            shaped.chars().count()
        );
        assert!(shaped.len() <= bound * 4, "{} bytes", shaped.len());
        assert!(
            shaped.chars().count() >= 128 * CHARS_PER_COLUMN,
            "the budget is spent before clusters are cut"
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
                    .row(display, row, columns.clone(), &mut self.system);
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
            .row(&text.map.snapshot(), 0, 0..128, &mut frames.system);
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
            (&one, "a\\xFE"),
            (&other, "a\\xFF"),
            (&replacement, "a\u{fffd}"),
        ]
        .into_iter()
        .enumerate()
        {
            rows.begin(id, revision as u64);
            rows.row(&buffer.display(), 0, 0..128, &mut system);
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
        let styles = |row: &ShapedRow| row.glyphs.iter().map(|g| g.style).collect::<Vec<_>>();
        let first = styles(rows.row(&escape.display(), 0, 0..128, &mut system));
        rows.begin(id, 2);
        let second = styles(rows.row(&typed.display(), 0, 0..128, &mut system));
        assert_eq!(first, [CellStyle::Escape; 4]);
        assert_eq!(second, [CellStyle::Text; 4]);
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
                rows.row(&display, row, 0..256, &mut system);
            }
            for shaped in &system.shaped {
                for c in shaped.chars() {
                    let bidi = matches!(u32::from(c), 0x61c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069);
                    proptest::prop_assert!(!c.is_control() && !bidi, "{c:?} in {shaped:?}");
                }
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
            let new = fresh.row(display, row, columns.clone(), &mut system);
            let old = &cached.rows[&row];
            proptest::prop_assert_eq!(&old.glyphs, &new.glyphs, "row {}", row);
            proptest::prop_assert_eq!(&old.boxes, &new.boxes, "row {}", row);
        }
        Ok(())
    }

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
