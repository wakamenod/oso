//! The cursor boxes, outlines and thin lines a pane draws: the primary cursor's box, or a hollow
//! one's sides, the other cursors' outlines (ADR 0091), a masked prompt's cursor cell, the
//! divider between side by side windows (ADR 0112), a strike's line and the line under a link.
//!
//! Must not depend on: `capture`, `paint` or a font backend.

use std::ops::Range;

use oso_core::frame::{CellRect, PixelRect};
use oso_platform::text::CellMetrics;

use super::highlight::RowCells;
use super::{Draws, panel};
use crate::pane::{Face, Masked, TextView};
use crate::text::Layout;

/// The line between side by side windows: one pixel, on a whole pixel, down the middle of
/// `region`'s column (ADR 0112).
pub(super) fn draw_divider(draws: &mut Draws, layout: &Layout, region: CellRect) {
    if let Some(rect) = layout.frame_cells(region) {
        let x = (rect.x + (rect.width - 1.0) / 2.0).round();
        let line = PixelRect {
            x,
            width: 1.0,
            ..rect
        };
        draws.fill(Some(line), Face::Divider);
    }
}

/// A thin line's width in whole pixels, as the metrics give the cell's height.
pub(super) fn line_width(metrics: CellMetrics) -> f32 {
    panel::point(metrics.height).round().max(1.0)
}

/// The four sides of `rect`, `width` thick, inside it.
pub(super) fn outline(rect: PixelRect, width: f32) -> [PixelRect; 4] {
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

/// Draws the outlines of `view`'s other cursors, over the glyphs and under the primary's
/// box (ADR 0091).
pub(super) fn outline_others(
    draws: &mut Draws,
    view: &TextView<'_>,
    layout: &Layout,
    metrics: CellMetrics,
) {
    let line = line_width(metrics);
    for rect in view
        .others
        .iter()
        .filter_map(|cells| layout.frame_cells(*cells))
    {
        for side in outline(rect, line) {
            draws.fill(Some(side), Face::SecondaryCursor);
        }
    }
}

/// Fills the cursor box over `cells`, or only its outline for a hollow one.
pub(super) fn fill_cursor(
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
pub(super) fn masked_cursor(masked: &Masked<'_>, region: CellRect) -> Option<CellRect> {
    (region.rows > 0 && masked.cursor < region.columns).then(|| CellRect {
        row: region.row,
        column: region.column + masked.cursor,
        rows: 1,
        columns: 1,
    })
}

/// A line `line` thick through the middle of `rect`, on a whole pixel: a strike, a terminal's
/// (ADR 0120) or deprecated code's (ADR 0138).
pub(super) fn through(rect: PixelRect, line: f32) -> PixelRect {
    PixelRect {
        y: (rect.y + (rect.height - line) / 2.0).round(),
        height: line,
        ..rect
    }
}

/// Draws the lines under `links` of view row `row`, each its first column and cells, at the
/// cells' bottom, below what tells letters apart, and behind the glyphs: in a link's colour, or where a fill in `plain` is, in the
/// text's, as the glyphs are.
pub(super) fn underline_links(
    draws: &mut Draws,
    (layout, row): (&Layout, usize),
    links: &[(usize, usize)],
    mut plain: RowCells<'_>,
    metrics: CellMetrics,
) {
    let line = line_width(metrics);
    let mut under = |columns: Range<usize>, face| {
        let rect = layout.display_cells(row, columns.start, columns.len());
        let rect = rect.map(|rect| PixelRect {
            // Of what shows of the cells, as a ruled pane's text ends above its line.
            y: rect.y + rect.height - line,
            height: line,
            ..rect
        });
        let ink = draws.theme.foreground(face);
        draws.fill_color(rect, ink);
    };
    for &(column, cells) in links {
        let [_, first] = layout.frame_cell(row, column);
        let (mut start, mut face) = (0, Face::SessionLink);
        for cell in 0..cells {
            let at = first + i64::try_from(cell).unwrap_or(i64::MAX);
            let now = if plain.meets(&(at..at + 1)) {
                Face::Text
            } else {
                Face::SessionLink
            };
            if now != face {
                under(column + start..column + cell, face);
                (start, face) = (cell, now);
            }
        }
        under(column + start..column + cells, face);
    }
}
