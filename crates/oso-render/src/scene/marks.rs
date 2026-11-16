//! What a language server's diagnostics draw (ADR 0138): a wavy line under a range in its
//! severity's colour, a line through deprecated code, faded text where code is unnecessary,
//! and a sign column's bars in the gutter.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - The lines are drawn over the fills and under the glyphs and the cursor box, each run of
//!   cells as one quad of the wave pipeline, within [`MAX_WAVES`], so a frame costs as many as
//!   the runs in view, never the buffer's diagnostics.
//! - A wave's phase counts from the frame's left, so runs side by side join, and its size
//!   follows the cell's: a thin line's thickness, swinging over three of them, a period of twice
//!   that.

use std::ops::Range;

use oso_core::frame::{CellRect, PixelRect};
use oso_platform::text::CellMetrics;

use super::highlight::RowCells;
use super::{Draws, line_width};
use crate::pane::{Face, TextMarks};
use crate::shape::{MAX_WAVES, Wave};
use crate::text::Layout;

/// Where one row's text is faded: in the faded cells, but where it keeps [`Face::Text`]'s
/// colour, on a fill or under the flash, as a syntax face does (ADR 0111). None by default.
#[derive(Clone, Copy, Default)]
pub(super) struct Fade<'a> {
    pub(super) faded: RowCells<'a>,
    pub(super) plain: RowCells<'a>,
}

impl Fade<'_> {
    /// Whether a glyph in the frame's columns `cells` is faded. Asked in rising columns.
    pub(super) fn covers(&mut self, cells: &Range<i64>) -> bool {
        let faded = self.faded.meets(cells);
        let plain = self.plain.meets(cells);
        faded && !plain
    }
}

impl Draws {
    /// Adds a wavy line in `face`'s foreground at the bottom of `rect`, a row's cells, if it
    /// shows and there is room.
    fn wave(&mut self, rect: Option<PixelRect>, face: Face, metrics: CellMetrics) {
        let Some(rect) = rect.filter(|_| self.waves.len() < MAX_WAVES) else {
            return;
        };
        let thickness = line_width(metrics);
        let band = (3.0 * thickness).min(rect.height);
        let y = rect.y + rect.height - band;
        self.waves.push(Wave {
            rect: [rect.x, y, rect.width, band],
            color: self.theme.foreground(face),
            params: [thickness, 2.0 * band, 0.0, 0.0],
            bounds: [rect.x, y, rect.x + rect.width, y + band],
        });
    }
}

/// Each of `cells`' rows on its own, as a wave or a strike is drawn on each row.
fn rows_of(cells: &CellRect) -> impl Iterator<Item = CellRect> + '_ {
    (cells.row..cells.row.saturating_add(cells.rows)).map(|row| CellRect {
        row,
        rows: 1,
        ..*cells
    })
}

/// Draws `marks`' wavy lines and strikes in `layout`, the pane's unscrolled one, as its
/// frame cells give them.
pub(super) fn draw_marks(
    draws: &mut Draws,
    marks: &TextMarks<'_>,
    layout: &Layout,
    metrics: CellMetrics,
) {
    for &(face, cells) in marks.underlines {
        for cells in cells.iter().flat_map(rows_of) {
            draws.wave(layout.frame_cells(cells), face, metrics);
        }
    }
    let line = line_width(metrics);
    let ink = draws.theme.foreground(Face::Text);
    for cells in marks.struck.iter().flat_map(rows_of) {
        // Through the middle of the cells, as a terminal's strike is drawn (ADR 0120).
        let rect = layout.frame_cells(cells).map(|rect| PixelRect {
            y: (rect.y + (rect.height - line) / 2.0).round(),
            height: line,
            ..rect
        });
        draws.fill_color(rect, ink);
    }
}

/// Draws a sign column's bars from `region`'s top row: each a third of a cell wide, at least a
/// pixel, at its cell's left, the row's full height.
pub(super) fn draw_signs(
    draws: &mut Draws,
    signs: &[Option<Face>],
    layout: &Layout,
    region: CellRect,
) {
    for (row, face) in signs.iter().enumerate().take(region.rows) {
        let Some(face) = face else {
            continue;
        };
        let cell = CellRect {
            row: region.row + row,
            column: region.column,
            rows: 1,
            columns: 1,
        };
        let rect = layout.frame_cells(cell).map(|rect| PixelRect {
            width: (rect.width / 3.0).round().max(1.0),
            ..rect
        });
        draws.fill_color(
            rect.filter(|_| region.columns > 0),
            draws.theme.foreground(*face),
        );
    }
}

#[cfg(test)]
mod tests;
