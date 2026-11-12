//! A jump's labels over a text pane (ADR 0116): the text's glyphs they hide, and their fills and
//! keys, drawn over everything else the pane draws, in its layer, so a float still covers them.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - The labels come sorted by row, then column. A row finds its own with a search, and its
//!   glyphs, which come in rising columns, walk them once.
//! - A label's keys are printable ASCII, one a cell. Each is shaped once, alone, and kept until
//!   the font changes, so a frame of labels shapes nothing. Anything else in a label's text
//!   draws no glyph.

use std::ops::Range;

use oso_core::frame::CellRect;
use oso_core::window::ViewTop;
use oso_platform::text::{CellMetrics, ShapedGlyph, TextSystem};

use super::{Draws, GlyphSource};
use crate::grid::{Grapheme, PlacedGlyph, Placer};
use crate::pane::{Face, Pane, Span};
use crate::text::Layout;

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

/// Draws `labels` over the text of `pane`, drawn with `metrics`: each one's cells in its face's
/// background, cut to the pane like the fills, and its keys in its face's foreground, one a
/// cell from its first, wherever the pane's view is scrolled and its cursor box is.
pub(super) fn draw_labels<G: GlyphSource>(
    draws: &mut Draws,
    labels: &[(CellRect, Span<'_>)],
    (pane, metrics): (&Pane<'_>, CellMetrics),
    system: &mut dyn TextSystem,
    glyphs: &mut G,
) {
    let region = pane.region;
    let layout = Layout::new(metrics, region, ViewTop::default()).with_bands(pane.bands);
    for (cells, span) in labels {
        draws.fill(layout.frame_cells(*cells), span.face);
        // A label on no row of the pane shows no key, nor do its keys past the pane's columns.
        let Some(row) = (cells.row.checked_sub(region.row)).filter(|row| *row < region.rows) else {
            continue;
        };
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
                if draws.quads.len() >= draws.limits.quads {
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
                if let Some(quad) = layout.quad(row, &glyph, &entry, &draws.theme) {
                    draws.quads.push(quad);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
