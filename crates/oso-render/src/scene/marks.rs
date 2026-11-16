//! What a language server's diagnostics draw (ADR 0138): a wavy line under a range in its
//! severity's colour, a line through deprecated code, faded text where code is unnecessary,
//! and a sign column's bars in the gutter.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - The lines are drawn over the fills and under the glyphs, but in the cursor's cells, which
//!   its box or a hollow one's sides keep, each run of cells as one quad of the wave pipeline,
//!   within [`MAX_WAVES`], so a frame costs as many as the runs in view, never the buffer's
//!   diagnostics. The caller gives the most severe first, so it keeps its room.
//! - Strikes and bars are rectangles, under the glyphs, as a terminal's strikes are.
//! - A wave's phase counts from the frame's left, so runs side by side join, and its size
//!   follows the cell's: a thin line's thickness, swinging over three of them, a period of twice
//!   that.

use std::ops::Range;

use oso_core::frame::{CellRect, PixelRect};
use oso_platform::text::CellMetrics;

use super::boxes::through;
use super::highlight::RowCells;
use super::{Draws, line_width};
use crate::pane::{Face, TextMarks};
use crate::shape::{MAX_WAVES, Wave};
use crate::text::Layout;

/// Where one row's text is faded: in the faded cells, but where it keeps [`Face::Text`]'s
/// colour, on a fill or under the flash, as a syntax face does (ADR 0111), and under the cursor
/// box, whose colours its glyphs take: the frame's columns of the box on the row.
pub(super) struct Fade<'a> {
    pub(super) faded: RowCells<'a>,
    pub(super) plain: RowCells<'a>,
    pub(super) cursor: Option<Range<i64>>,
}

impl Fade<'_> {
    /// Whether a glyph in the frame's columns `cells` is faded. Asked in rising columns.
    pub(super) fn covers(&mut self, cells: &Range<i64>) -> bool {
        let faded = self.faded.meets(cells);
        let plain = self.plain.meets(cells);
        let boxed = (self.cursor.as_ref())
            .is_some_and(|cursor| cursor.start < cells.end && cells.start < cursor.end);
        faded && !plain && !boxed
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
    (layout, cursor): (&Layout, Option<CellRect>),
    metrics: CellMetrics,
) {
    for &(face, cells) in marks.underlines {
        for cells in cells.iter().flat_map(rows_of) {
            // Not over the cursor's cells: its box, or a hollow one's sides, stays whole.
            for cells in around(cells, cursor).into_iter().flatten() {
                draws.wave(layout.frame_cells(cells), face, metrics);
            }
        }
    }
    let line = line_width(metrics);
    let ink = draws.theme.foreground(Face::Text);
    for cells in marks.struck.iter().flat_map(rows_of) {
        let rect = layout.frame_cells(cells).map(|rect| through(rect, line));
        draws.fill_color(rect, ink);
    }
}

/// What is left of `cells`, one row's, around `cut`: the cells before it and those after it.
fn around(cells: CellRect, cut: Option<CellRect>) -> [Option<CellRect>; 2] {
    let Some(cut) = cut.filter(|cut| (cut.row..cut.row + cut.rows).contains(&cells.row)) else {
        return [Some(cells), None];
    };
    let (start, end) = (cells.column, cells.column + cells.columns);
    let (cut_start, cut_end) = (cut.column, cut.column + cut.columns);
    if cut_end <= start || end <= cut_start {
        return [Some(cells), None];
    }
    let part = |from: usize, to: usize| {
        (from < to).then_some(CellRect {
            column: from,
            columns: to - from,
            ..cells
        })
    };
    [part(start, cut_start), part(cut_end, end)]
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
