//! What a frame draws: a list of panes turned into rectangles and glyph quads,
//! layer by layer (ADR 0061). No GPU is touched here, so all of it is tested
//! with a fake text system and atlas.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - Each layer is one range of rectangles, one of rounded boxes and one of quads,
//!   drawn back to front in that order, so a frame costs one draw per layer per
//!   pipeline, however many panes and glyphs it has. A layer with no rounded box
//!   draws none.
//! - Panes are built front to back, the reverse of how they are drawn. The caps
//!   on quads and rectangles and the budget of new glyphs then run out behind
//!   what is in front, so no amount of text in a pane can take the place of what
//!   is drawn over it or after it. A full atlas can leave the rebuild short of
//!   budget, which the next frame makes up, the front first again.
//! - A full atlas clears, which leaves the quads built so far pointing at glyphs
//!   that are gone, so every pane is built again.

use std::ops::Range;

use oso_core::frame::{self, CellRect, PixelRect};
use oso_core::motion;
use oso_core::window::Scroll;
use oso_platform::text::{CellMetrics, GlyphKey, TextSystem};

use crate::atlas::AtlasGlyph;
use crate::pane::{CursorStyle, Face, Layer, Masked, Pane, PaneContent, PaneId, Span, TextView};
use crate::rows::{Rows, ShapedRow};
use crate::shape::{MAX_ROUNDED, Rect, Rounded};
use crate::text::{GlyphInstance, Layout};
use crate::theme::Theme;

mod panel;

/// Where a frame's glyphs come from, as an atlas gives them.
pub(crate) trait GlyphSource {
    /// A glyph's place, rasterizing it if needed, or `None` if it has no
    /// bitmap or must wait for a later frame.
    fn get(&mut self, system: &mut dyn TextSystem, key: GlyphKey) -> Option<AtlasGlyph>;
    /// Whether a glyph did not fit since the last call.
    fn take_full(&mut self) -> bool;
    /// Drops every glyph.
    fn clear(&mut self);
}

/// The most a frame draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) rects: usize,
    pub(crate) quads: usize,
}

/// One layer's part of the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayerDraw {
    pub(crate) rects: Range<u32>,
    pub(crate) rounded: Range<u32>,
    pub(crate) quads: Range<u32>,
}

/// What a frame has built so far.
struct Draws {
    limits: Limits,
    rects: Vec<Rect>,
    rounded: Vec<Rounded>,
    quads: Vec<GlyphInstance>,
    /// Rectangles held back for what is filled last, such as a cursor box.
    held: usize,
    /// What colours the faces take (ADR 0089). Switching it reshapes and rasterizes nothing.
    theme: Theme,
}

impl Draws {
    /// Fills `rect`, if it shows and there is room.
    fn fill(&mut self, rect: Option<PixelRect>, face: Face) {
        let room = self.rects.len() + self.held < self.limits.rects;
        if let Some(rect) = rect.filter(|_| room) {
            self.rects.push(Rect {
                position: [rect.x, rect.y],
                size: [rect.width, rect.height],
                color: self.theme.background(face),
            });
        }
    }

    /// Adds what `row` shows. Returns false once there is no room for more.
    fn row<G: GlyphSource>(
        &mut self,
        layout: &Layout,
        row: usize,
        shaped: &ShapedRow,
        system: &mut dyn TextSystem,
        glyphs: &mut G,
    ) -> bool {
        for (i, &(column, cells)) in shaped.boxes.iter().enumerate() {
            // A box followed by another ends a pixel short, so two never read as one
            // (ADR 0051, ADR 0089).
            let joined = shaped
                .boxes
                .get(i + 1)
                .is_some_and(|&(next, _)| next == column + cells);
            let rect = layout
                .display_cells(row, column, cells)
                .map(|rect| PixelRect {
                    width: rect.width - if joined { 1.0 } else { 0.0 },
                    ..rect
                });
            self.fill(rect.filter(|rect| rect.width > 0.0), Face::Boxed);
        }
        // Only glyphs in the pane's columns are looked up, so nothing out of
        // view is rasterized: a glyph paints only its own cells.
        let columns = layout.columns();
        let from = shaped
            .glyphs
            .partition_point(|g| g.column + (g.cells as usize) <= columns.start);
        for glyph in &shaped.glyphs[from..] {
            if glyph.column >= columns.end {
                break;
            }
            if self.quads.len() >= self.limits.quads {
                return false;
            }
            let Some(entry) = glyphs.get(system, glyph.key) else {
                continue;
            };
            if let Some(quad) = layout.quad(row, glyph, &entry, &self.theme) {
                self.quads.push(quad);
            }
        }
        true
    }
}

/// A frame's rectangles and quads, built from its panes, and each pane's shaped
/// rows. Everything is reused across frames.
pub(crate) struct Scene {
    /// By pane. A frame's list is short, so a search costs less than a map.
    rows: Vec<(PaneId, Rows)>,
    /// The panes of this frame from the back, as indices into its list.
    order: Vec<usize>,
    draws: Draws,
    /// This frame's layers, from the back.
    layers: Vec<LayerDraw>,
}

impl Scene {
    pub(crate) fn new(limits: Limits, theme: Theme) -> Self {
        Self {
            rows: Vec::new(),
            order: Vec::new(),
            draws: Draws {
                limits,
                rects: Vec::new(),
                rounded: Vec::new(),
                quads: Vec::new(),
                held: 0,
                theme,
            },
            layers: Vec::new(),
        }
    }

    /// Draws the next frame in `theme`'s colours. Nothing is shaped or rasterized again.
    pub(crate) fn set_theme(&mut self, theme: Theme) {
        self.draws.theme = theme;
    }

    pub(crate) fn theme(&self) -> &Theme {
        &self.draws.theme
    }

    /// Forgets every pane's shaping, for when the font changes.
    pub(crate) fn invalidate(&mut self) {
        for (_, rows) in &mut self.rows {
            rows.invalidate();
        }
    }

    /// Rows shaped so far by the panes still here, for logs.
    pub(crate) fn shaped_count(&self) -> u64 {
        self.rows.iter().map(|(_, rows)| rows.shaped_count()).sum()
    }

    pub(crate) fn rects(&self) -> &[Rect] {
        &self.draws.rects
    }

    pub(crate) fn rounded(&self) -> &[Rounded] {
        &self.draws.rounded
    }

    pub(crate) fn quads(&self) -> &[GlyphInstance] {
        &self.draws.quads
    }

    /// The layers to draw, back to front.
    pub(crate) fn layers(&self) -> &[LayerDraw] {
        &self.layers
    }

    /// Builds the frame of `panes`.
    pub(crate) fn build<G: GlyphSource>(
        &mut self,
        panes: &[Pane<'_>],
        system: &mut dyn TextSystem,
        glyphs: &mut G,
    ) {
        debug_assert!(
            panes
                .iter()
                .enumerate()
                .all(|(i, a)| panes[..i].iter().all(|b| a.id != b.id)),
            "panes in a list share a PaneId, and with it their shaped rows"
        );
        debug_assert!(
            panes.iter().enumerate().all(|(i, a)| {
                panes[..i]
                    .iter()
                    .all(|b| a.layer != b.layer || !overlap(a.region, b.region))
            }),
            "panes in one layer overlap, so one's rectangles would not hide the other's text"
        );
        // The sort keeps the list's order inside a layer.
        self.order.clear();
        self.order.extend(0..panes.len());
        self.order.sort_by_key(|&index| panes[index].layer);
        self.rows
            .retain(|(id, _)| panes.iter().any(|pane| pane.id == *id));
        let metrics = system.metrics();
        // A frame that fills the atlas clears it and is built once more, so a
        // full atlas never leaves glyphs invisible for good.
        for attempt in 0..2 {
            self.build_once(panes, metrics, system, glyphs);
            if !glyphs.take_full() {
                break;
            }
            if attempt == 0 {
                log::debug!("glyph atlas is full; clearing it");
                glyphs.clear();
            }
        }
    }

    fn build_once<G: GlyphSource>(
        &mut self,
        panes: &[Pane<'_>],
        metrics: CellMetrics,
        system: &mut dyn TextSystem,
        glyphs: &mut G,
    ) {
        self.draws.rects.clear();
        self.draws.rounded.clear();
        self.draws.quads.clear();
        self.layers.clear();
        let mut current = None;
        for &index in self.order.iter().rev() {
            let pane = &panes[index];
            if current != Some(pane.layer) {
                current = Some(pane.layer);
                let rects = self.draws.rects.len() as u32;
                let rounded = self.draws.rounded.len() as u32;
                let quads = self.draws.quads.len() as u32;
                self.layers.push(LayerDraw {
                    rects: rects..rects,
                    rounded: rounded..rounded,
                    quads: quads..quads,
                });
            }
            let rows = rows_of(&mut self.rows, pane.id);
            draw_pane(&mut self.draws, pane, metrics, rows, system, glyphs);
            if let Some(layer) = self.layers.last_mut() {
                layer.rects.end = self.draws.rects.len() as u32;
                layer.rounded.end = self.draws.rounded.len() as u32;
                layer.quads.end = self.draws.quads.len() as u32;
            }
        }
        self.layers.reverse();
    }
}

/// Whether two regions share a cell. An empty region shares none.
fn overlap(a: CellRect, b: CellRect) -> bool {
    let along = |start_a: usize, len_a: usize, start_b: usize, len_b: usize| {
        len_a > 0
            && len_b > 0
            && start_a < start_b.saturating_add(len_b)
            && start_b < start_a.saturating_add(len_a)
    };
    along(a.row, a.rows, b.row, b.rows) && along(a.column, a.columns, b.column, b.columns)
}

/// The rows of pane `id`, new ones if it has none.
fn rows_of(rows: &mut Vec<(PaneId, Rows)>, id: PaneId) -> &mut Rows {
    let index = match rows.iter().position(|(pane, _)| *pane == id) {
        Some(index) => index,
        None => {
            rows.push((id, Rows::default()));
            rows.len() - 1
        }
    };
    &mut rows[index].1
}

fn draw_pane<G: GlyphSource>(
    draws: &mut Draws,
    pane: &Pane<'_>,
    metrics: CellMetrics,
    rows: &mut Rows,
    system: &mut dyn TextSystem,
    glyphs: &mut G,
) {
    let unscrolled = Layout::new(metrics, pane.region, Scroll::default());
    // A text pane's cursor box is filled last, over its background and boxes, and
    // none of them can take its place.
    let cursor = match &pane.content {
        PaneContent::Text(view) => cursor_cells(view, pane.region),
        PaneContent::Masked(masked) => masked_cursor(masked, pane.region),
        PaneContent::Lines(_) => None,
    };
    let hollow = matches!(&pane.content, PaneContent::Text(view) if view.cursor_style == CursorStyle::Hollow);
    // So are a preedit's two underlines, and a hollow box's four sides.
    let held = cursor.map_or(0, |_| if hollow { 4 } else { 1 }) + preedit_lines(pane);
    draws.held += held;
    if let Some(face) = pane.background {
        draws.fill(unscrolled.frame_cells(pane.region), face);
    }
    if let Some(panel) = &pane.panel {
        let room = MAX_ROUNDED.saturating_sub(draws.rounded.len());
        let cell = [metrics.width, metrics.height];
        draws
            .rounded
            .extend(panel::boxes(panel, pane.region, cell, &draws.theme).take(room));
    }
    match &pane.content {
        PaneContent::Text(view) => {
            // Behind the boxes and the glyphs, so the text keeps its face, back to front.
            for &(face, cells) in view.fills {
                for cells in cells {
                    draws.fill(unscrolled.frame_cells(*cells), face);
                }
            }
            let mut layout = Layout::new(metrics, pane.region, view.scroll).with_bands(pane.bands);
            if let Some(cells) = cursor.filter(|_| !hollow) {
                // Glyphs take the box's color only where the box is.
                layout = layout.with_cursor(view.cursor.row, cells);
            }
            draw_text(draws, view, pane, &layout, rows, system, glyphs);
            draws.held -= held;
            underline_preedit(draws, view, &layout, metrics);
            if let Some(cells) = cursor {
                fill_cursor(draws, &layout, cells, hollow, metrics);
            }
        }
        PaneContent::Lines(lines) => {
            let rows_shown = pane.region.rows;
            // A modeline or an echo area. Other panes keep what their `bands` say.
            let layout = if pane.layer == Layer::TILED && rows_shown == 1 {
                unscrolled.with_band_above()
            } else {
                unscrolled.with_bands(pane.bands)
            };
            draw_lines(draws, lines, &layout, rows_shown, rows, system, glyphs);
        }
        PaneContent::Masked(masked) => {
            let mut layout = unscrolled.with_bands(pane.bands);
            if let Some(cells) = cursor {
                layout = layout.with_cursor(0, cells);
            }
            let span = [Span {
                text: masked.text,
                face: Face::Float,
            }];
            let line: [&[Span<'_>]; 1] = [&span];
            draw_lines(
                draws,
                &line,
                &layout,
                pane.region.rows.min(1),
                rows,
                system,
                glyphs,
            );
            draws.held -= held;
            if let Some(cells) = cursor {
                fill_cursor(draws, &layout, cells, false, metrics);
            }
        }
    }
}

/// A thin line's width in whole pixels, as the metrics give the cell's height.
fn line_width(metrics: CellMetrics) -> f32 {
    panel::point(metrics.height).round().max(1.0)
}

/// The four sides of `rect`, `width` thick, inside it.
fn outline(rect: PixelRect, width: f32) -> [PixelRect; 4] {
    let width = width.min(rect.width / 2.0).min(rect.height / 2.0);
    [
        PixelRect {
            height: width,
            ..rect
        },
        PixelRect {
            y: rect.y + rect.height - width,
            height: width,
            ..rect
        },
        PixelRect { width, ..rect },
        PixelRect {
            x: rect.x + rect.width - width,
            width,
            ..rect
        },
    ]
}

/// Fills the cursor box over `cells`, or only its outline for a hollow one.
fn fill_cursor(
    draws: &mut Draws,
    layout: &Layout,
    cells: CellRect,
    hollow: bool,
    metrics: CellMetrics,
) {
    let Some(rect) = layout.frame_cells(cells) else {
        return;
    };
    if hollow {
        for side in outline(rect, line_width(metrics)) {
            draws.fill(Some(side), Face::Cursor);
        }
    } else {
        draws.fill(Some(rect), Face::Cursor);
    }
}

/// The frame's cell under the cursor of `masked` in `region`, if it is in it.
fn masked_cursor(masked: &Masked<'_>, region: CellRect) -> Option<CellRect> {
    (region.rows > 0 && masked.cursor < region.columns).then(|| CellRect {
        row: region.row,
        column: region.column + masked.cursor,
        rows: 1,
        columns: 1,
    })
}

/// The frame's cells under the cursor box of `view` in `region`, if it shows.
/// The box covers the whole character under it, two cells for a full-width one
/// (ADR 0026), and goes where a preedit puts it (ADR 0069).
fn cursor_cells(view: &TextView<'_>, region: CellRect) -> Option<CellRect> {
    let (point, cells) = motion::cursor_box(&view.display, view.cursor, view.preedit);
    frame::cursor_rect(region, view.scroll, point, cells)
}

/// The rectangles a pane's preedit underlines take.
fn preedit_lines(pane: &Pane<'_>) -> usize {
    match &pane.content {
        PaneContent::Text(view) if view.preedit.is_some() => 2,
        _ => 0,
    }
}

/// Underlines `view`'s preedit, one line thick, and the segment being converted
/// two lines thick, at the bottom of its cells, as macOS text views draw them
/// (ADR 0069).
fn underline_preedit(
    draws: &mut Draws,
    view: &TextView<'_>,
    layout: &Layout,
    metrics: CellMetrics,
) {
    let Some(preedit) = view.preedit else {
        return;
    };
    let line = line_width(metrics);
    let at = view.cursor.column;
    let mut underline = |column: usize, cells: usize, thickness: f32| {
        let cells = layout.display_cells(view.cursor.row, column, cells);
        let rect = cells.map(|rect| PixelRect {
            y: rect.y + rect.height - thickness,
            height: thickness,
            ..rect
        });
        draws.fill(rect, Face::Preedit);
    };
    let (all, segment) = preedit.underlines(at);
    underline(all.start, all.len(), line);
    if let Some(segment) = segment {
        underline(segment.start, segment.len(), 2.0 * line);
    }
}

/// Draws a window's view of a buffer in `layout`: the rows of its region.
fn draw_text<G: GlyphSource>(
    draws: &mut Draws,
    view: &TextView,
    pane: &Pane<'_>,
    layout: &Layout,
    rows: &mut Rows,
    system: &mut dyn TextSystem,
    glyphs: &mut G,
) {
    let display = &view.display;
    rows.set_whitespace(view.whitespace);
    rows.begin(view.buffer, view.revision);
    let columns = layout.shaped_columns();
    let top = view.scroll.top;
    let visible = top..top
        .saturating_add(pane.region.rows)
        .min(display.row_count());
    let preedit = view.preedit.map(|preedit| (view.cursor.column, preedit));
    for row in visible.clone() {
        let spliced = preedit.filter(|_| row == view.cursor.row);
        let shaped = rows.row(display, row, columns.clone(), spliced, system);
        if !draws.row(layout, row, shaped, system, glyphs) {
            break;
        }
    }
    rows.evict(visible.start.saturating_sub(visible.len())..visible.end + visible.len());
}

/// Draws rows that are no buffer's, from the region's top row.
fn draw_lines<G: GlyphSource>(
    draws: &mut Draws,
    lines: &[&[Span<'_>]],
    layout: &Layout,
    shown: usize,
    rows: &mut Rows,
    system: &mut dyn TextSystem,
    glyphs: &mut G,
) {
    rows.begin_lines();
    let end = layout.columns().end;
    for (row, spans) in lines.iter().enumerate().take(shown) {
        let shaped = rows.line(row, spans, end, system);
        if !draws.row(layout, row, shaped, system, glyphs) {
            break;
        }
    }
    rows.evict(0..lines.len().min(shown));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::{Bands, Panel};
    use crate::rows::tests::Recorder;
    use oso_core::buffer::{Buffer, BufferId};
    use oso_core::display_map::DisplayPoint;
    use oso_core::frame::FrameSize;
    use oso_platform::text::GlyphFormat;

    const LIMITS: Limits = Limits {
        rects: 1000,
        quads: 1000,
    };

    /// An atlas with a place for every glyph, 8 x 12 px on the baseline. Its
    /// first coordinate says which clearing the glyph is from, so a stale one
    /// shows, and it can run out of room or budget once.
    #[derive(Default)]
    struct FakeAtlas {
        generation: u32,
        looked_up: Vec<GlyphKey>,
        /// Reports full once, after this many glyphs.
        full_after: Option<usize>,
        full: bool,
        /// How many glyphs it has before the rest must wait.
        budget: Option<usize>,
        /// The bitmap's size, left and top, if not the usual one.
        bitmap: Option<([u32; 2], i32, i32)>,
    }

    impl GlyphSource for FakeAtlas {
        fn get(&mut self, _system: &mut dyn TextSystem, key: GlyphKey) -> Option<AtlasGlyph> {
            self.looked_up.push(key);
            if self.full_after == Some(self.looked_up.len()) {
                self.full = true;
            }
            if let Some(budget) = &mut self.budget {
                *budget = budget.checked_sub(1)?;
            }
            let (size, left, top) = self.bitmap.unwrap_or(([8, 12], 1, 12));
            Some(AtlasGlyph {
                format: GlyphFormat::Coverage,
                uv: [self.generation, 0],
                size,
                left,
                top,
            })
        }

        fn take_full(&mut self) -> bool {
            std::mem::take(&mut self.full)
        }

        fn clear(&mut self) {
            self.generation += 1;
        }
    }

    fn theme() -> Theme {
        crate::theme::themes()[0]
    }

    fn rect(row: usize, column: usize, rows: usize, columns: usize) -> CellRect {
        CellRect {
            row,
            column,
            rows,
            columns,
        }
    }

    fn id() -> BufferId {
        oso_core::editor::Editor::default().window().buffer()
    }

    fn view(buffer: &Buffer) -> PaneContent<'static> {
        PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(1000, 0),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills: &[],
            whitespace: false,
        })
    }

    fn pane<'a>(id: u32, region: CellRect, layer: u8, content: PaneContent<'a>) -> Pane<'a> {
        Pane {
            id: PaneId(id),
            region,
            layer: Layer(layer),
            background: None,
            panel: None,
            bands: Bands::NONE,
            content,
        }
    }

    fn build(scene: &mut Scene, panes: &[Pane<'_>], atlas: &mut FakeAtlas) -> Recorder {
        let mut system = Recorder::default();
        scene.build(panes, &mut system, atlas);
        system
    }

    /// The quads' top-left corners, as `(row, column)` of the 10 x 20 px grid
    /// with a margin of 8 px. The fake glyph sits 1 px in and 4 px down.
    fn quad_cells(scene: &Scene, quads: Range<u32>) -> Vec<(i32, i32)> {
        scene.quads()[quads.start as usize..quads.end as usize]
            .iter()
            .map(|q| {
                let [x, y] = q.position();
                ((y as i32 - 12) / 20, (x as i32 - 9) / 10)
            })
            .collect()
    }

    /// A line of `text` in the text face.
    fn line(text: &str) -> [Span<'_>; 1] {
        [Span {
            text,
            face: Face::Text,
        }]
    }

    #[test]
    fn only_a_tiled_row_that_is_no_buffers_keeps_the_band_above_it() {
        // A bitmap reaching far above its row shows how far up a pane may draw.
        let top_of = |panes: &[Pane<'_>]| {
            let mut atlas = FakeAtlas {
                bitmap: Some(([8, 60], 1, 60)),
                ..FakeAtlas::default()
            };
            let mut scene = Scene::new(LIMITS, theme());
            build(&mut scene, panes, &mut atlas);
            scene.quads()[0].position()[1]
        };
        let spans = line("x");
        let lines: [&[Span<'_>]; 1] = [&spans];
        let buffer = Buffer::from_text("t", "x");
        // Row 2 starts at y 48, and the band is 4 px.
        let one_row = rect(2, 0, 1, 10);
        let tiled = pane(0, one_row, 0, PaneContent::Lines(&lines));
        assert_eq!(top_of(&[tiled]), 44.0);
        let taller = pane(0, rect(2, 0, 2, 10), 0, PaneContent::Lines(&lines));
        assert_eq!(top_of(&[taller]), 48.0);
        let in_front = pane(0, one_row, 1, PaneContent::Lines(&lines));
        assert_eq!(top_of(&[in_front]), 48.0);
        let text = pane(0, one_row, 0, view(&buffer));
        assert_eq!(top_of(&[text]), 48.0);
    }

    proptest::proptest! {
        /// Whatever the regions, the scroll position and the glyphs' bitmaps,
        /// everything a pane draws stays in its region, but for the band above
        /// the frame's top row or a tiled row that is no buffer's.
        #[test]
        fn prop_a_pane_draws_only_in_its_region(
            (row, column, rows, columns) in (0usize..6, 0usize..12, 0usize..6, 0usize..12),
            (top, left) in (0usize..8, 0usize..130),
            (bitmap_left, bitmap_top) in (-40i32..40, -40i32..40),
            (width, height) in (1u32..60, 1u32..60),
            lines in proptest::bool::ANY,
        ) {
            let region = rect(row, column, rows, columns);
            let text = "abcあd\u{202e}e\n".repeat(10);
            let buffer = Buffer::from_text("t", &text);
            let spans = line("ab\u{202e}あ");
            let spans: [&[Span<'_>]; 1] = [&spans];
            let content = if lines {
                PaneContent::Lines(&spans)
            } else {
                PaneContent::Text(TextView {
                    display: buffer.display(),
                    buffer: id(),
                    revision: 1,
                    scroll: Scroll { top, left },
                    cursor: DisplayPoint::new(top + 1, left + 1),
                    cursor_style: CursorStyle::Box,
                    preedit: None,
                    fills: &[],
                    whitespace: false,
                })
            };
            let mut panes = [pane(0, region, 0, content)];
            panes[0].background = Some(Face::Boxed);
            let mut atlas = FakeAtlas {
                bitmap: Some(([width, height], bitmap_left, bitmap_top)),
                ..FakeAtlas::default()
            };
            let mut scene = Scene::new(LIMITS, theme());
            build(&mut scene, &panes, &mut atlas);
            let (x0, y0) = (8.0 + column as f32 * 10.0, 8.0 + row as f32 * 20.0);
            let (x1, y1) = (x0 + columns as f32 * 10.0, y0 + rows as f32 * 20.0);
            let band = if row == 0 || (rows == 1 && lines) { 4.0 } else { 0.0 };
            for rect in scene.rects() {
                let ([x, y], [w, h]) = (rect.position, rect.size);
                proptest::prop_assert!(x >= x0 && y >= y0 && x + w <= x1 && y + h <= y1);
                proptest::prop_assert!(w > 0.0 && h > 0.0);
            }
            for quad in scene.quads() {
                let ([x, y], [w, h]) = (quad.position(), quad.size());
                proptest::prop_assert!(x >= x0 && y >= y0 - band && x + w <= x1 && y + h <= y1);
                proptest::prop_assert!(w > 0.0 && h > 0.0);
            }
        }
    }

    #[test]
    fn a_text_area_draws_exactly_its_rows() {
        let text = "x\n".repeat(2000);
        let buffer = Buffer::from_text("t", &text);
        for frame_rows in [0, 1, 2, 3, 40, 1000] {
            let area = frame::layout(FrameSize {
                rows: frame_rows,
                columns: 80,
            })
            .window
            .text;
            let mut scene = Scene::new(LIMITS, theme());
            let panes = [pane(0, area, 0, view(&buffer))];
            let system = build(&mut scene, &panes, &mut FakeAtlas::default());
            assert_eq!(system.shaped.len(), area.rows, "a frame of {frame_rows}");
            assert_eq!(scene.quads().len(), area.rows, "a frame of {frame_rows}");
        }
    }

    #[test]
    fn a_pane_draws_only_in_its_region() {
        let buffer = Buffer::from_text("t", &"abcdefgh\n".repeat(10));
        let panes = [pane(0, rect(2, 3, 2, 4), 0, view(&buffer))];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let cells = quad_cells(&scene, 0..scene.quads().len() as u32);
        let mut expected = Vec::new();
        for row in 2..4 {
            for column in 3..7 {
                expected.push((row, column));
            }
        }
        assert_eq!(cells, expected);
    }

    #[test]
    fn scrolled_out_cells_are_not_drawn_or_looked_up() {
        let buffer = Buffer::from_text("t", &"abcdefgh\n".repeat(10));
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll { top: 3, left: 64 },
            cursor: DisplayPoint::new(1000, 0),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills: &[],
            whitespace: false,
        });
        let panes = [pane(0, rect(0, 0, 2, 4), 0, content)];
        let (mut scene, mut atlas) = (Scene::new(LIMITS, theme()), FakeAtlas::default());
        build(&mut scene, &panes, &mut atlas);
        assert!(scene.quads().is_empty());
        assert!(atlas.looked_up.is_empty());
    }

    #[test]
    fn panes_in_one_layer_are_one_draw_and_each_layer_is_its_own() {
        let (a, b) = (line("aa"), line("bbb"));
        let (a, b): ([&[Span<'_>]; 1], [&[Span<'_>]; 1]) = ([&a], [&b]);
        let panes = [
            pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&a)),
            pane(1, rect(1, 0, 1, 10), 0, PaneContent::Lines(&b)),
            pane(2, rect(0, 0, 1, 10), 3, PaneContent::Lines(&b)),
        ];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let layers = scene.layers();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].quads.len(), 5, "the tiled layer's two panes");
        assert_eq!(layers[1].quads.len(), 3, "the layer in front");
        assert_eq!(scene.quads().len(), 8);
    }

    #[test]
    fn layers_are_drawn_from_the_back_whatever_the_list_order() {
        let (back, front) = (line("b"), line("ff"));
        let (back, front): ([&[Span<'_>]; 1], [&[Span<'_>]; 1]) = ([&back], [&front]);
        let panes = [
            pane(0, rect(0, 0, 1, 10), 5, PaneContent::Lines(&front)),
            pane(1, rect(0, 0, 1, 10), 0, PaneContent::Lines(&back)),
        ];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        assert_eq!(scene.layers()[0].quads.len(), 1);
        assert_eq!(scene.layers()[1].quads.len(), 2);
    }

    #[test]
    fn each_layer_holds_its_own_rectangles_and_text() {
        let text = line("x");
        let text: [&[Span<'_>]; 1] = [&text];
        let boxed = Buffer::from_text("t", "\u{202e}");
        let mut panes = [
            pane(0, rect(0, 0, 3, 10), 0, view(&boxed)),
            pane(1, rect(1, 2, 1, 4), 1, PaneContent::Lines(&text)),
        ];
        panes[1].background = Some(Face::Text);
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let (back, front) = (&scene.layers()[0], &scene.layers()[1]);
        let colors = |rects: &Range<u32>| -> Vec<[f32; 4]> {
            scene.rects()[rects.start as usize..rects.end as usize]
                .iter()
                .map(|r| r.color)
                .collect()
        };
        assert_eq!(colors(&back.rects), [theme().background(Face::Boxed)]);
        assert_eq!(colors(&front.rects), [theme().background(Face::Text)]);
        assert_eq!(quad_cells(&scene, front.quads.clone()), [(1, 2)]);
        // The opaque background is the whole region, in pixels.
        let filled = scene.rects()[front.rects.start as usize];
        assert_eq!(filled.position, [28.0, 28.0]);
        assert_eq!(filled.size, [40.0, 20.0]);
    }

    #[test]
    fn a_panel_is_rounded_boxes_cut_to_the_region_and_the_margin_above_it() {
        let keys = [rect(3, 4, 1, 3), rect(3, 8, 1, 3)];
        let panes = [Pane {
            panel: Some(Panel {
                cells: rect(0, 2, 4, 10),
                field: Some(rect(2, 4, 1, 6)),
                field_room: rect(1, 3, 3, 8),
                keys: &keys,
                selected: None,
                border: false,
            }),
            ..pane(0, rect(0, 0, 6, 12), 16, PaneContent::Lines(&[]))
        }];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        assert!(scene.rects().is_empty(), "no background, no border");
        let rounded = scene.rounded();
        assert_eq!(rounded.len(), 4, "the panel, its field and two keys");
        assert_eq!(scene.layers()[0].rounded, 0..4);
        // Cells are 10 x 20 px from (8, 8), and a point is 20 / 16 px. On the top row the
        // panel reaches the surface's edge, with its top corners square.
        let panel = rounded[0];
        assert_eq!(panel.rect, [28.0, 0.0, 100.0, 88.0]);
        assert_eq!(panel.radii, [0.0, 0.0, 10.0, 10.0]);
        // Every quad is cut to the region, and to the margin above it on the top row.
        for r in rounded {
            let [x0, y0, x1, y1] = r.bounds;
            assert!(
                x0 >= 8.0 && y0 >= 0.0 && x1 <= 128.0 && y1 <= 128.0,
                "{r:?}"
            );
        }
        assert_eq!(panel.bounds[1], 0.0);
        assert!(panel.shadow[3] > 0.0 && panel.params[2] > 0.0);
        // The field reaches past its cells and its outline ends there.
        let field = rounded[1];
        assert_eq!(
            field.rect,
            [48.0 - 7.0, 48.0 - 4.0, 60.0 + 14.0, 20.0 + 8.0]
        );
        assert_eq!(field.fill, theme().background(Face::Field));
        assert_eq!(field.outline, theme().foreground(Face::Field));
        assert_eq!(rounded[2].fill, theme().background(Face::Key));
    }

    #[test]
    fn a_field_reaches_only_into_its_room_and_a_box_out_of_the_region_is_dropped() {
        let keys = [rect(9, 4, 1, 3)];
        let panes = [Pane {
            panel: Some(Panel {
                cells: rect(0, 2, 2, 10),
                // No gap and no padding: the field keeps to its row.
                field: Some(rect(1, 4, 1, 6)),
                field_room: rect(1, 4, 1, 6),
                keys: &keys,
                selected: None,
                border: false,
            }),
            ..pane(0, rect(0, 0, 2, 12), 16, PaneContent::Lines(&[]))
        }];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let rounded = scene.rounded();
        assert_eq!(rounded.len(), 2, "the key below the region is dropped");
        // The row's cells are x 48..108 and y 28..48, the outline 1 px inside them.
        assert_eq!(rounded[1].rect, [49.0, 29.0, 58.0, 18.0]);
        // The shadow stops at the region's bottom, y 48.
        assert_eq!(rounded[0].bounds[3], 48.0);
    }

    #[test]
    fn the_selected_candidate_is_a_box_a_little_wider_than_its_row_inside_the_panel() {
        let panes = [Pane {
            panel: Some(Panel {
                cells: rect(0, 1, 6, 10),
                field: None,
                field_room: rect(0, 0, 0, 0),
                keys: &[],
                selected: Some(rect(4, 2, 1, 8)),
                border: false,
            }),
            ..pane(0, rect(0, 0, 8, 12), 16, PaneContent::Lines(&[]))
        }];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let rounded = scene.rounded();
        assert_eq!(rounded.len(), 2);
        let [panel, row] = [rounded[0], rounded[1]];
        assert_eq!(row.fill, theme().background(Face::CompletionSelected));
        // The row's cells are x 28..108; the box reaches past them, and stays in the panel.
        let [x, y, width, height] = row.rect;
        let [px, _, pwidth, _] = panel.rect;
        assert!(x < 28.0 && x >= px && x + width > 108.0 && x + width <= px + pwidth);
        assert_eq!((y, height), (8.0 + 4.0 * 20.0, 20.0));
    }

    #[test]
    fn a_panel_with_a_border_has_it_in_place_of_the_hairline() {
        let panes = [Pane {
            panel: Some(Panel {
                cells: rect(2, 2, 4, 8),
                field: None,
                field_room: rect(0, 0, 0, 0),
                keys: &[],
                selected: Some(rect(4, 3, 1, 6)),
                border: true,
            }),
            ..pane(0, rect(0, 0, 8, 12), 16, PaneContent::Lines(&[]))
        }];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let rounded = scene.rounded();
        // The border is a square box behind the panel, 5 points of a 20 px cell: 6.25 px, in
        // whole pixels, past it on every side.
        let (frame, panel) = (rounded[0], rounded[1]);
        assert_eq!(
            (frame.fill, frame.radii),
            (theme().panel().border, [0.0; 4])
        );
        assert_eq!(frame.rect[0], panel.rect[0] - 6.0);
        assert_eq!(frame.rect[2], panel.rect[2] + 12.0);
        // Square, on the text's background, with no shadow and no outline of its own.
        assert_eq!(panel.radii, [0.0; 4]);
        assert_eq!(panel.fill, theme().background(Face::Text));
        assert_eq!((panel.shadow[3], panel.params[0]), (0.0, 0.0));
        // The selected row is a bar across the whole panel.
        let bar = rounded[2];
        assert_eq!(bar.fill, theme().background(Face::CompletionSelected));
        assert_eq!((bar.rect[0], bar.rect[2]), (panel.rect[0], panel.rect[2]));
        assert_eq!(bar.radii, [0.0; 4]);
    }

    #[test]
    fn a_shadow_with_little_room_below_shrinks_to_end_inside_its_pane() {
        let panes = [Pane {
            panel: Some(Panel {
                cells: rect(0, 2, 2, 10),
                field: None,
                field_room: rect(0, 0, 0, 0),
                keys: &[],
                selected: None,
                border: false,
            }),
            ..pane(0, rect(0, 0, 3, 12), 16, PaneContent::Lines(&[]))
        }];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let panel = scene.rounded()[0];
        // The panel ends at y 48 and the pane at 68: the shadow's reach fits in 20 px.
        let [_, down, sigma, _] = panel.params;
        assert!(3.0 * sigma + down <= 20.0 + 1e-3, "{sigma} {down}");
        assert!(panel.shadow[3] < theme().panel().shadow[3]);
    }

    #[test]
    fn a_frame_with_no_panel_has_no_rounded_box() {
        let lines: [&[Span<'_>]; 1] = [&line("abc")];
        let panes = [Pane {
            background: Some(Face::Text),
            ..pane(0, rect(0, 0, 3, 10), 0, PaneContent::Lines(&lines))
        }];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        assert!(scene.rounded().is_empty());
        assert!(scene.layers().iter().all(|layer| layer.rounded.is_empty()));
    }

    #[test]
    fn a_hollow_cursor_is_an_outline_over_glyphs_as_they_are() {
        let buffer = Buffer::from_text("t", "abc");
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(0, 1),
            cursor_style: CursorStyle::Hollow,
            preedit: None,
            fills: &[],
            whitespace: false,
        });
        let panes = [pane(0, rect(0, 0, 1, 3), 0, content)];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let rects = scene.rects();
        assert_eq!(rects.len(), 4);
        assert!(
            rects
                .iter()
                .all(|r| r.color == theme().background(Face::Cursor))
        );
        assert_eq!(
            (rects[0].position, rects[0].size),
            ([18.0, 8.0], [10.0, 1.0])
        );
        assert_eq!(
            (rects[2].position, rects[2].size),
            ([18.0, 8.0], [1.0, 20.0])
        );
        assert_eq!(scene.quads().len(), 3, "every glyph still shows");
    }

    #[test]
    fn a_masked_row_draws_what_stands_for_the_text_and_a_cursor_box() {
        let content = PaneContent::Masked(Masked {
            text: "***",
            cursor: 3,
        });
        let panes = [pane(0, rect(0, 0, 1, 5), 17, content)];
        let mut scene = Scene::new(LIMITS, theme());
        let system = build(&mut scene, &panes, &mut FakeAtlas::default());
        assert_eq!(scene.quads().len(), 3);
        assert!(
            system
                .shaped
                .iter()
                .all(|text| text.chars().all(|c| c == '*'))
        );
        let last = scene.rects().last().unwrap();
        assert_eq!(last.color, theme().background(Face::Cursor));
        assert_eq!((last.position, last.size), ([38.0, 8.0], [10.0, 20.0]));
    }

    #[test]
    fn a_pane_keeps_only_the_bands_it_is_given() {
        let line = line("A");
        let lines: [&[Span<'_>]; 1] = [&line];
        // Bits above and below the row: [top, bottom] of the one quad, by the bands kept.
        let mut reach = Vec::new();
        for bands in [
            Bands::NONE,
            Bands {
                above: true,
                below: false,
            },
            Bands {
                above: true,
                below: true,
            },
        ] {
            let panes = [Pane {
                bands,
                ..pane(0, rect(2, 0, 1, 4), 17, PaneContent::Lines(&lines))
            }];
            let mut atlas = FakeAtlas {
                bitmap: Some(([8, 40], 1, 22)),
                ..FakeAtlas::default()
            };
            let mut scene = Scene::new(LIMITS, theme());
            build(&mut scene, &panes, &mut atlas);
            let quad = &scene.quads()[0];
            reach.push([quad.position()[1], quad.position()[1] + quad.size()[1]]);
        }
        // The row spans y 48 to 68; nothing past it without a band, in any layer.
        assert_eq!(reach[0], [48.0, 68.0], "{reach:?}");
        assert!(reach[1][0] < 48.0 && reach[1][1] == 68.0, "{reach:?}");
        assert!(reach[2][0] < 48.0 && reach[2][1] > 68.0, "{reach:?}");
    }

    #[test]
    fn the_cursor_box_keeps_its_rectangle_when_boxes_fill_the_rest() {
        // Every character is a 6-cell box, more of them than the frame may draw.
        let buffer = Buffer::from_text("t", &"\u{202e}".repeat(20));
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(0, 0),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills: &[],
            whitespace: false,
        });
        let panes = [pane(0, rect(0, 0, 1, 120), 0, content)];
        let limits = Limits {
            rects: 8,
            quads: 1000,
        };
        let mut scene = Scene::new(limits, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let rects = scene.rects();
        assert_eq!(rects.len(), limits.rects, "the boxes took all they could");
        assert!(
            rects[..rects.len() - 1]
                .iter()
                .all(|r| r.color == theme().background(Face::Boxed))
        );
        let last = rects.last().unwrap();
        assert_eq!(last.color, theme().background(Face::Cursor));
        assert_eq!((last.position, last.size), ([8.0, 8.0], [60.0, 20.0]));
    }

    #[test]
    fn the_cursor_box_goes_over_the_boxes_and_stays_in_the_pane() {
        let buffer = Buffer::from_text("t", "\u{202e}abc");
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(0, 0),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills: &[],
            whitespace: false,
        });
        let panes = [pane(0, rect(1, 1, 1, 3), 0, content)];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let last = scene.rects().last().unwrap();
        assert_eq!(last.color, theme().background(Face::Cursor));
        // The cursor is on a 6-cell label, cut to the 3 cells of the region.
        assert_eq!(last.position, [18.0, 28.0]);
        assert_eq!(last.size, [30.0, 20.0]);
        let boxed = scene.rects()[0];
        assert_eq!(boxed.size, [30.0, 20.0], "the box is cut too");
    }

    #[test]
    fn the_region_goes_behind_the_boxes_and_the_cursor_and_stays_in_the_pane() {
        let buffer = Buffer::from_text("t", "\u{202e}abc\nde");
        let region = [rect(1, 1, 1, 8), rect(2, 1, 1, 20)];
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(1, 1),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills: &[(Face::Region, &region)],
            whitespace: false,
        });
        let mut panes = [pane(0, rect(1, 1, 2, 8), 0, content)];
        panes[0].background = Some(Face::Text);
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let rects: Vec<_> = scene
            .rects()
            .iter()
            .map(|rect| (rect.position, rect.size, rect.color))
            .collect();
        assert_eq!(
            rects,
            [
                ([18.0, 28.0], [80.0, 40.0], theme().background(Face::Text)),
                ([18.0, 28.0], [80.0, 20.0], theme().background(Face::Region)),
                ([18.0, 48.0], [80.0, 20.0], theme().background(Face::Region)),
                ([18.0, 28.0], [60.0, 20.0], theme().background(Face::Boxed)),
                ([28.0, 48.0], [10.0, 20.0], theme().background(Face::Cursor)),
            ],
            "the second row of the region is cut to the pane"
        );
    }

    #[test]
    fn a_preedit_is_underlined_its_segment_more_thickly_and_the_box_goes_after_it() {
        use oso_core::display_map::Preedit;
        let buffer = Buffer::from_text("t", "abc");
        // Converting な, the second of two cells of kana.
        let preedit = Preedit::new("かな".into(), Some(3..6));
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(0, 1),
            cursor_style: CursorStyle::Box,
            preedit: Some(&preedit),
            fills: &[],
            whitespace: false,
        });
        let panes = [pane(0, rect(0, 0, 1, 10), 0, content)];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let rects: Vec<_> = scene
            .rects()
            .iter()
            .map(|rect| (rect.position, rect.size, rect.color))
            .collect();
        let line = theme().background(Face::Preedit);
        assert_eq!(
            rects,
            [
                ([18.0, 27.0], [40.0, 1.0], line),
                ([38.0, 26.0], [20.0, 2.0], line),
                ([58.0, 8.0], [10.0, 20.0], theme().background(Face::Cursor)),
            ],
            "the box is on b, pushed to column 5"
        );
        assert_eq!(
            quad_cells(&scene, 0..scene.quads().len() as u32),
            [(0, 0), (0, 1), (0, 3), (0, 5), (0, 6)]
        );
    }

    #[test]
    fn a_switch_of_theme_shapes_and_rasterizes_nothing_again() {
        let buffer = Buffer::from_text("t", "text\u{202e}");
        let content = || {
            PaneContent::Text(TextView {
                display: buffer.display(),
                buffer: id(),
                revision: 1,
                scroll: Scroll::default(),
                cursor: DisplayPoint::new(0, 0),
                cursor_style: CursorStyle::Box,
                preedit: None,
                fills: &[],
                whitespace: false,
            })
        };
        let mut scene = Scene::new(LIMITS, theme());
        let mut atlas = FakeAtlas::default();
        build(
            &mut scene,
            &[pane(0, rect(0, 0, 2, 12), 0, content())],
            &mut atlas,
        );
        let (glyphs, before) = (atlas.looked_up.len(), scene.rects()[0].color);
        let other = theme().swapped();
        scene.set_theme(other);
        let system = build(
            &mut scene,
            &[pane(0, rect(0, 0, 2, 12), 0, content())],
            &mut atlas,
        );
        assert!(system.shaped.is_empty(), "{:?}", system.shaped);
        assert_eq!(
            atlas.looked_up.len(),
            2 * glyphs,
            "the same glyphs, from the atlas"
        );
        assert_eq!(scene.rects()[0].color, other.background(Face::Boxed));
        assert_ne!(scene.rects()[0].color, before);
        assert_eq!(scene.quads()[0].color(), other.foreground(Face::Cursor));
    }

    #[test]
    fn turning_whitespace_on_shapes_the_rows_again_and_draws_the_marks() {
        let buffer = Buffer::from_text("t", "a\tb");
        let content = |whitespace| {
            PaneContent::Text(TextView {
                display: buffer.display(),
                buffer: id(),
                revision: 1,
                scroll: Scroll::default(),
                cursor: DisplayPoint::new(1000, 0),
                cursor_style: CursorStyle::Box,
                preedit: None,
                fills: &[],
                whitespace,
            })
        };
        let mut scene = Scene::new(LIMITS, theme());
        let mut atlas = FakeAtlas::default();
        build(
            &mut scene,
            &[pane(0, rect(0, 0, 1, 20), 0, content(false))],
            &mut atlas,
        );
        let plain = scene.quads().len();
        // The same revision: only the switch changed, and the row is shaped again.
        let system = build(
            &mut scene,
            &[pane(0, rect(0, 0, 1, 20), 0, content(true))],
            &mut atlas,
        );
        assert!(
            system.shaped.iter().any(|piece| piece == "»"),
            "{:?}",
            system.shaped
        );
        assert_eq!(scene.quads().len(), plain + 1, "the tab's mark");
        let mark = scene.quads()[1].color();
        assert_eq!(mark, theme().foreground(Face::Whitespace));
    }

    #[test]
    fn a_box_followed_by_another_ends_a_pixel_short() {
        let buffer = Buffer::from_text("t", "\u{202e}\u{202e}a\u{202e}");
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(1000, 0),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills: &[],
            whitespace: false,
        });
        let mut scene = Scene::new(LIMITS, theme());
        build(
            &mut scene,
            &[pane(0, rect(0, 0, 1, 20), 0, content)],
            &mut FakeAtlas::default(),
        );
        let boxes: Vec<_> = scene
            .rects()
            .iter()
            .map(|r| (r.position[0], r.size[0]))
            .collect();
        // Two labels of 6 cells side by side, then a letter and a third, past the margin.
        assert_eq!(boxes, [(8.0, 59.0), (68.0, 60.0), (138.0, 60.0)]);
    }

    #[test]
    fn panes_keep_their_own_rows() {
        let (one, two) = (Buffer::from_text("t", "one"), Buffer::from_text("t", "two"));
        let mut editor = oso_core::editor::Editor::default();
        let second = editor.add_buffer(Buffer::from_text("t", ""));
        let content = |buffer: &Buffer, id: BufferId| {
            PaneContent::Text(TextView {
                display: buffer.display(),
                buffer: id,
                revision: 1,
                scroll: Scroll::default(),
                cursor: DisplayPoint::new(1000, 0),
                cursor_style: CursorStyle::Box,
                preedit: None,
                fills: &[],
                whitespace: false,
            })
        };
        let panes = |first, second_id| {
            [
                pane(0, rect(0, 0, 1, 10), 0, content(&one, first)),
                pane(1, rect(1, 0, 1, 10), 0, content(&two, second_id)),
            ]
        };
        let mut scene = Scene::new(LIMITS, theme());
        let mut atlas = FakeAtlas::default();
        // Two buffers side by side: if the panes shared a cache, each frame
        // would drop the other's rows and shape both again.
        let system = build(&mut scene, &panes(id(), second), &mut atlas);
        assert_eq!(system.shaped.len(), 2);
        for _ in 0..3 {
            let system = build(&mut scene, &panes(id(), second), &mut atlas);
            assert!(system.shaped.is_empty(), "{:?}", system.shaped);
        }
    }

    #[test]
    fn a_pane_gone_from_the_list_loses_its_rows() {
        let text = line("text");
        let text: [&[Span<'_>]; 1] = [&text];
        let with = [
            pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&text)),
            pane(1, rect(1, 0, 1, 10), 0, PaneContent::Lines(&text)),
        ];
        let (mut scene, mut atlas) = (Scene::new(LIMITS, theme()), FakeAtlas::default());
        build(&mut scene, &with, &mut atlas);
        let without = [pane(1, rect(1, 0, 1, 10), 0, PaneContent::Lines(&text))];
        let system = build(&mut scene, &without, &mut atlas);
        assert!(system.shaped.is_empty(), "pane 1 kept its rows");
        assert_eq!(scene.rows.len(), 1);
        // Back in the list, the pane starts over.
        let system = build(&mut scene, &with, &mut atlas);
        assert_eq!(system.shaped.len(), 1);
    }

    #[test]
    fn invalidating_shapes_every_pane_again() {
        let text = line("text");
        let text: [&[Span<'_>]; 1] = [&text];
        let panes = [
            pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&text)),
            pane(1, rect(1, 0, 1, 10), 0, PaneContent::Lines(&text)),
        ];
        let (mut scene, mut atlas) = (Scene::new(LIMITS, theme()), FakeAtlas::default());
        build(&mut scene, &panes, &mut atlas);
        scene.invalidate();
        let system = build(&mut scene, &panes, &mut atlas);
        assert_eq!(system.shaped.len(), 2);
        assert_eq!(scene.shaped_count(), 4);
    }

    #[test]
    fn a_full_atlas_rebuilds_every_pane_with_new_places() {
        let text = line("abcd");
        let text: [&[Span<'_>]; 1] = [&text];
        let panes = [
            pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&text)),
            pane(1, rect(1, 0, 1, 10), 0, PaneContent::Lines(&text)),
            pane(2, rect(0, 0, 1, 10), 1, PaneContent::Lines(&text)),
        ];
        // The atlas fills while the second of three panes is built.
        let mut atlas = FakeAtlas {
            full_after: Some(6),
            ..FakeAtlas::default()
        };
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut atlas);
        assert_eq!(atlas.generation, 1, "it was cleared once");
        assert_eq!(scene.quads().len(), 12, "nothing is drawn twice");
        for quad in scene.quads() {
            assert_eq!(quad.uv()[0], 1.0, "a glyph from before the clearing");
        }
        assert_eq!(scene.layers().len(), 2);
        assert_eq!(scene.layers()[0].quads.len(), 8);
    }

    #[test]
    fn text_below_cannot_take_the_place_of_what_is_in_front() {
        let long = "x".repeat(50);
        let long = line(&long);
        let warning = line("warn");
        let (long, warning): ([&[Span<'_>]; 1], [&[Span<'_>]; 1]) = ([&long], [&warning]);
        let panes = [
            // More glyphs than the frame has room for, first in the list.
            pane(0, rect(0, 0, 1, 50), 0, PaneContent::Lines(&long)),
            pane(1, rect(1, 0, 1, 50), 0, PaneContent::Lines(&warning)),
            pane(2, rect(2, 0, 1, 50), 1, PaneContent::Lines(&warning)),
        ];
        let limits = Limits {
            rects: 1000,
            quads: 20,
        };
        let mut scene = Scene::new(limits, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        assert_eq!(scene.quads().len(), 20);
        let (back, front) = (&scene.layers()[0], &scene.layers()[1]);
        assert_eq!(front.quads.len(), 4);
        assert_eq!(
            back.quads.len(),
            16,
            "4 of the row after it, and 12 of the rest"
        );
        let cells = quad_cells(&scene, back.quads.clone());
        assert_eq!(cells.iter().filter(|(row, _)| *row == 1).count(), 4);
    }

    #[test]
    fn the_budget_of_new_glyphs_goes_to_the_front_first() {
        let text = line("abcd");
        let text: [&[Span<'_>]; 1] = [&text];
        let panes = [
            pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&text)),
            pane(1, rect(0, 0, 1, 10), 1, PaneContent::Lines(&text)),
        ];
        let mut atlas = FakeAtlas {
            budget: Some(4),
            ..FakeAtlas::default()
        };
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut atlas);
        assert_eq!(scene.layers().len(), 2);
        assert!(scene.layers()[0].quads.is_empty(), "the back waits");
        assert_eq!(scene.layers()[1].quads.len(), 4);
    }

    #[test]
    fn too_many_rectangles_leave_out_those_behind() {
        let panes = [
            pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&[])),
            pane(1, rect(1, 0, 1, 10), 1, PaneContent::Lines(&[])),
        ];
        let [mut back, mut front] = panes;
        back.background = Some(Face::Text);
        front.background = Some(Face::Cursor);
        let limits = Limits {
            rects: 1,
            quads: 10,
        };
        let mut scene = Scene::new(limits, theme());
        build(&mut scene, &[back, front], &mut FakeAtlas::default());
        assert_eq!(scene.rects().len(), 1);
        assert_eq!(scene.rects()[0].color, theme().background(Face::Cursor));
    }

    #[test]
    fn a_background_and_boxes_cannot_take_the_place_of_the_cursor_box() {
        let buffer = Buffer::from_text("t", &"\u{200b}".repeat(20));
        let content = PaneContent::Text(TextView {
            display: buffer.display(),
            buffer: id(),
            revision: 1,
            scroll: Scroll::default(),
            cursor: DisplayPoint::new(0, 0),
            cursor_style: CursorStyle::Box,
            preedit: None,
            fills: &[],
            whitespace: false,
        });
        let mut panes = [pane(0, rect(0, 0, 2, 200), 0, content)];
        panes[0].background = Some(Face::Modeline);
        let limits = Limits {
            rects: 5,
            quads: 1000,
        };
        let mut scene = Scene::new(limits, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let colors: Vec<_> = scene.rects().iter().map(|r| r.color).collect();
        assert_eq!(colors.len(), 5);
        assert_eq!(colors[0], theme().background(Face::Modeline));
        assert_eq!(colors[1..4], [theme().background(Face::Boxed); 3]);
        assert_eq!(colors[4], theme().background(Face::Cursor));
    }

    #[test]
    fn a_pane_with_no_cursor_box_in_it_holds_no_rectangle() {
        let buffer = Buffer::from_text("t", &"\u{200b}".repeat(20));
        let panes = [pane(0, rect(0, 0, 2, 200), 0, view(&buffer))];
        let limits = Limits {
            rects: 5,
            quads: 1000,
        };
        let mut scene = Scene::new(limits, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let colors: Vec<_> = scene.rects().iter().map(|r| r.color).collect();
        assert_eq!(colors, [theme().background(Face::Boxed); 5]);
    }

    #[test]
    fn empty_panes_draw_nothing_and_shape_nothing() {
        let buffer = Buffer::from_text("t", "text");
        let spans = line("text");
        let spans: [&[Span<'_>]; 1] = [&spans];
        let mut panes = [
            pane(0, rect(0, 0, 0, 10), 0, view(&buffer)),
            pane(1, rect(0, 0, 3, 10), 0, PaneContent::Lines(&[])),
            pane(2, rect(0, 0, 0, 0), 0, PaneContent::Lines(&spans)),
        ];
        panes[2].background = Some(Face::Text);
        let mut scene = Scene::new(LIMITS, theme());
        let system = build(&mut scene, &panes, &mut FakeAtlas::default());
        assert!(system.shaped.is_empty());
        assert!(scene.quads().is_empty());
        assert!(scene.rects().is_empty());
    }

    #[test]
    fn regions_overlap_when_they_share_a_cell() {
        let region = rect(2, 2, 2, 2);
        assert!(overlap(region, rect(3, 3, 5, 5)));
        assert!(overlap(region, region));
        assert!(!overlap(region, rect(4, 2, 2, 2)), "the row below");
        assert!(!overlap(region, rect(2, 4, 2, 2)), "the column beside");
        assert!(!overlap(region, rect(2, 2, 0, 2)), "an empty one");
        assert!(overlap(region, rect(0, 0, usize::MAX, usize::MAX)));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "panes in one layer overlap")]
    fn panes_overlapping_in_one_layer_are_refused() {
        let panes = [
            pane(0, rect(0, 0, 2, 10), 0, PaneContent::Lines(&[])),
            pane(1, rect(1, 0, 2, 10), 0, PaneContent::Lines(&[])),
        ];
        build(
            &mut Scene::new(LIMITS, theme()),
            &panes,
            &mut FakeAtlas::default(),
        );
    }

    #[test]
    fn panes_may_overlap_across_layers() {
        let panes = [
            pane(0, rect(0, 0, 2, 10), 0, PaneContent::Lines(&[])),
            pane(1, rect(1, 0, 2, 10), 1, PaneContent::Lines(&[])),
        ];
        build(
            &mut Scene::new(LIMITS, theme()),
            &panes,
            &mut FakeAtlas::default(),
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "share a PaneId")]
    fn panes_sharing_an_id_are_refused() {
        let panes = [
            pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&[])),
            pane(0, rect(1, 0, 1, 10), 0, PaneContent::Lines(&[])),
        ];
        build(
            &mut Scene::new(LIMITS, theme()),
            &panes,
            &mut FakeAtlas::default(),
        );
    }

    #[test]
    fn no_panes_draw_nothing() {
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &[], &mut FakeAtlas::default());
        assert!(scene.layers().is_empty());
    }

    #[test]
    fn lines_past_the_region_are_not_read() {
        let (one, two) = (line("one"), line("two"));
        let lines: [&[Span<'_>]; 2] = [&one, &two];
        let panes = [pane(0, rect(0, 0, 1, 10), 0, PaneContent::Lines(&lines))];
        let mut scene = Scene::new(LIMITS, theme());
        let system = build(&mut scene, &panes, &mut FakeAtlas::default());
        assert_eq!(system.shaped, ["one"]);
    }

    #[test]
    fn a_line_stops_at_its_regions_right_edge() {
        let text = line("abcdefgh");
        let text: [&[Span<'_>]; 1] = [&text];
        let panes = [pane(0, rect(0, 2, 1, 3), 0, PaneContent::Lines(&text))];
        let mut scene = Scene::new(LIMITS, theme());
        build(&mut scene, &panes, &mut FakeAtlas::default());
        let cells = quad_cells(&scene, 0..scene.quads().len() as u32);
        assert_eq!(cells, [(0, 2), (0, 3), (0, 4)]);
    }
}
