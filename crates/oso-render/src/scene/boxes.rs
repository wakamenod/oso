//! The cursor boxes, outlines and thin lines a pane draws: the primary cursor's box, or a hollow
//! one's sides, the other cursors' outlines (ADR 0091), a masked prompt's cursor cell and the
//! divider between side by side windows (ADR 0112).
//!
//! Must not depend on: `capture`, `paint` or a font backend.

use oso_core::frame::{CellRect, PixelRect};
use oso_platform::text::CellMetrics;

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
