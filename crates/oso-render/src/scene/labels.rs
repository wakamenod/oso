//! A jump's labels over a text pane (ADR 0116): the text's glyphs they hide, and their fills and
//! keys, drawn over everything else in the pane's layer, so a float still covers them.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - The labels come sorted by row, then column. A row finds its own with a search, and its
//!   glyphs, which come in rising columns, walk them once.
//! - A label's keys are printable ASCII, one a cell. Each is shaped once, alone, and kept until
//!   the font changes, so a frame of labels shapes nothing. Anything else in a label's text
//!   draws no glyph.
//! - What the labels draw is held back while their layer is built ([`Over`]), and goes in a
//!   layer of its own just in front of it, so no glyph of another row or pane paints over a
//!   label.

use std::ops::Range;

use oso_core::frame::{CellRect, PixelRect};
use oso_core::window::ViewTop;
use oso_platform::text::{CellMetrics, ShapedGlyph, TextSystem};

use super::{Draws, GlyphSource, LayerDraw};
use crate::grid::{Grapheme, PlacedGlyph, Placer};
use crate::pane::{Face, Pane, Span};
use crate::shape::Rect;
use crate::text::{GlyphInstance, Layout};

/// The characters a label's key may draw: printable ASCII but the space, which draws nothing.
const KEYS: Range<u8> = b'!'..b'\x7f';

/// How many [`KEYS`] there are.
const KEY_COUNT: usize = (KEYS.end - KEYS.start) as usize;

/// The labels on one frame row, from the first that may still cover a glyph.
pub(super) struct Covered<'a>(&'a [(CellRect, Span<'a>)]);

impl<'a> Covered<'a> {
    /// None: for text no label is drawn over.
    pub(super) const NONE: Covered<'static> = Covered(&[]);

    /// The labels of `labels`, sorted by row, then column, that are on frame row `row`.
    pub(super) fn on(labels: &'a [(CellRect, Span<'a>)], row: i64) -> Self {
        let row_of = |cells: &CellRect| i64::try_from(cells.row).unwrap_or(i64::MAX);
        let start = labels.partition_point(|(cells, _)| row_of(cells) < row);
        let rest = &labels[start..];
        Self(&rest[..rest.partition_point(|(cells, _)| row_of(cells) <= row)])
    }

    /// Whether a label covers any of the frame's columns `columns` on the row. Asked in rising
    /// starts, as a row's glyphs come, so the labels left of them are done with.
    pub(super) fn any(&mut self, columns: &Range<i64>) -> bool {
        let span = |cells: &CellRect| {
            let start = i64::try_from(cells.column).unwrap_or(i64::MAX);
            start..start.saturating_add(i64::try_from(cells.columns).unwrap_or(i64::MAX))
        };
        while let Some(((cells, _), rest)) = self.0.split_first()
            && span(cells).end <= columns.start
        {
            self.0 = rest;
        }
        // The first left starts at or before any other, so if it starts past them, all do.
        self.0
            .first()
            .is_some_and(|(cells, _)| span(cells).start < columns.end)
    }
}

/// What a layer's labels draw, in the order they are drawn.
#[derive(Default)]
pub(super) struct Over {
    pub(super) rects: Vec<Rect>,
    pub(super) quads: Vec<GlyphInstance>,
}

impl Draws {
    /// Fills `rect` over everything else in the layer, as a label is, if there is room.
    /// Returns whether it did.
    fn fill_over(&mut self, rect: PixelRect, face: Face) -> bool {
        let room = self.room_for_rect();
        if room {
            self.over.rects.push(self.filled(rect, face));
        }
        room
    }

    /// Ends the layer last begun in `layers`, which are built front to back: what its labels
    /// drew goes after all of it, in a layer of its own just in front of it.
    pub(super) fn end_layer(&mut self, layers: &mut Vec<LayerDraw>) {
        if self.over.rects.is_empty() && self.over.quads.is_empty() {
            return;
        }
        let (rects, rounded, quads) = (
            self.rects.len() as u32,
            self.rounded.len() as u32,
            self.quads.len() as u32,
        );
        self.rects.append(&mut self.over.rects);
        self.quads.append(&mut self.over.quads);
        let labels = LayerDraw {
            rects: rects..self.rects.len() as u32,
            rounded: rounded..rounded,
            quads: quads..self.quads.len() as u32,
        };
        layers.insert(layers.len().saturating_sub(1), labels);
    }
}

/// The glyphs of the label keys shaped so far, each placed in one cell from column 0. Kept
/// from frame to frame, until the font changes.
pub(super) struct Keys {
    /// By key, from the first of [`KEYS`]: where its glyphs are in `glyphs`, once shaped.
    placed: [Option<(u32, u32)>; KEY_COUNT],
    glyphs: Vec<PlacedGlyph>,
    /// Scratch space for shaping and placing a key.
    shaped: Vec<ShapedGlyph>,
    one: Vec<PlacedGlyph>,
    placer: Placer,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            placed: [None; KEY_COUNT],
            glyphs: Vec::new(),
            shaped: Vec::new(),
            one: Vec::new(),
            placer: Placer::default(),
        }
    }
}

impl Keys {
    /// Forgets every key's glyphs, for when the font changes.
    pub(super) fn clear(&mut self) {
        self.placed = [None; KEY_COUNT];
        self.glyphs.clear();
    }

    /// Where the glyphs of `key` are in `glyphs`, shaping it the first time, or `None` if it is
    /// no key a label draws.
    fn get(&mut self, system: &mut dyn TextSystem, key: char) -> Option<Range<usize>> {
        let byte = u8::try_from(key).ok().filter(|byte| KEYS.contains(byte))?;
        let slot = usize::from(byte - KEYS.start);
        if let Some((start, end)) = self.placed[slot] {
            return Some(start as usize..end as usize);
        }
        let mut text = [0; 4];
        system.shape(key.encode_utf8(&mut text), &mut self.shaped);
        let one = [Grapheme {
            byte: 0,
            column: 0,
            cells: 1,
            face: Face::JumpLabel,
        }];
        let width = system.metrics().width;
        self.placer.place(&one, &self.shaped, width, &mut self.one);
        let start = self.glyphs.len();
        self.glyphs.extend_from_slice(&self.one);
        let end = self.glyphs.len();
        // At most a few glyphs for each of the few keys, so these never overflow.
        self.placed[slot] = Some((start as u32, end as u32));
        Some(start..end)
    }
}

/// Draws `labels` over the text of `pane`, drawn with `metrics`, and over the rest of its layer:
/// each one's cells in its face's background, cut to the pane like the fills, and its keys in its
/// face's foreground, one a cell from its first and cut to its cells, wherever the pane's view is
/// scrolled and its cursor box is. A label whose fill finds no room draws no key.
pub(super) fn draw_labels<G: GlyphSource>(
    draws: &mut Draws,
    labels: &[(CellRect, Span<'_>)],
    (pane, metrics): (&Pane<'_>, CellMetrics),
    system: &mut dyn TextSystem,
    glyphs: &mut G,
) {
    debug_assert!(
        labels.is_sorted_by_key(|(cells, _)| (cells.row, cells.column)),
        "labels come sorted by row, then column"
    );
    let region = pane.region;
    let layout = Layout::new(metrics, region, ViewTop::default()).with_bands(pane.bands);
    for (cells, span) in labels {
        // A label on no row of the pane shows nothing, nor do its keys past the pane's columns.
        let Some(fill) = layout.frame_cells(*cells) else {
            continue;
        };
        let Some(row) = (cells.row.checked_sub(region.row)).filter(|row| *row < region.rows) else {
            continue;
        };
        if !draws.fill_over(fill, span.face) {
            return;
        }
        let columns = region.column..region.column.saturating_add(region.columns);
        for (index, key) in span.text.chars().take(cells.columns).enumerate() {
            let column = cells.column.saturating_add(index);
            if !columns.contains(&column) {
                continue;
            }
            let Some(placed) = draws.keys.get(system, key) else {
                continue;
            };
            for at in placed {
                if !draws.room_for_quad() {
                    return;
                }
                let glyph = PlacedGlyph {
                    column: column - region.column,
                    face: span.face,
                    ..draws.keys.glyphs[at]
                };
                let Some(entry) = glyphs.get(system, glyph.key) else {
                    continue;
                };
                let quad = layout.quad(row, &glyph, &entry, &draws.theme);
                if let Some(quad) = quad.and_then(|quad| quad.cut_to(fill)) {
                    draws.over.quads.push(quad);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
