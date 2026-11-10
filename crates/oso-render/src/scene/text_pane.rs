//! What a pane shows of a buffer: its screen rows, each from its own line's column, with the
//! wrap mark on those that continue, the cursor box's cells and a preedit's underlines on each
//! row it takes (ADR 0069, ADR 0100, ADR 0104).
//!
//! Must not depend on: `capture`, `paint` or a font backend.

use std::ops::Range;

use oso_core::frame::{self, CellRect, PixelRect};
use oso_core::motion;
use oso_platform::text::{CellMetrics, TextSystem};

use super::{Draws, GlyphSource, line_width};
use crate::pane::{Face, Pane, PaneContent, TextView};
use crate::rows::Rows;
use crate::text::Layout;

/// The frame's cells under the cursor box of `view` in `region`, if it shows.
/// The box covers the whole character under it, two cells for a full-width one
/// (ADR 0026), and goes where a preedit puts it (ADR 0069).
pub(super) fn cursor_cells(view: &TextView<'_>, region: CellRect) -> Option<CellRect> {
    let (point, cells) = motion::cursor_box(&view.display, view.cursor, view.preedit);
    frame::cursor_rect(region, view.screen, point, cells)
}

/// The rectangles a pane's preedit underlines take: two on each row of its line, which it
/// may wrap over (ADR 0100).
pub(super) fn preedit_lines(pane: &Pane<'_>) -> usize {
    match &pane.content {
        PaneContent::Text(view) if view.preedit.is_some() => {
            let rows = view.screen.rows().iter().take(pane.region.rows);
            2 * rows.filter(|row| row.line == view.cursor.row).count()
        }
        _ => 0,
    }
}

/// Underlines `view`'s preedit, one line thick, and the segment being converted
/// two lines thick, at the bottom of its cells, as macOS text views draw them
/// (ADR 0069).
pub(super) fn underline_preedit(
    draws: &mut Draws,
    view: &TextView<'_>,
    layout: &Layout,
    metrics: CellMetrics,
) {
    let Some(preedit) = view.preedit else {
        return;
    };
    let line = line_width(metrics);
    let (all, segment) = preedit.underlines(view.cursor.column);
    let screen = view.screen;
    for (index, row) in screen.rows().iter().enumerate() {
        if row.line != view.cursor.row {
            continue;
        }
        // Each row underlines the part of the preedit it shows.
        let row_layout = layout.with_left(screen.origin(row));
        let mut underline = |columns: Range<usize>, thickness: f32| {
            let (start, end) = (
                columns.start.max(row.column),
                columns.end.min(row.end_column),
            );
            if start >= end {
                return;
            }
            let cells = row_layout.display_cells(index, start, end - start);
            let rect = cells.map(|rect| PixelRect {
                y: rect.y + rect.height - thickness,
                height: thickness,
                ..rect
            });
            draws.fill(rect, Face::Preedit);
        };
        underline(all.clone(), line);
        if let Some(segment) = segment.clone() {
            underline(segment, 2.0 * line);
        }
    }
}

/// Draws a window's view of a buffer in `layout`: the rows of its region.
pub(super) fn draw_text<G: GlyphSource>(
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
    let screen = view.screen;
    // From the window's left column; 0 while wrapped.
    let shaped = layout.shaped_columns();
    let preedit = view.preedit.map(|preedit| (view.cursor.column, preedit));
    let shown = &screen.rows()[..screen.rows().len().min(pane.region.rows)];
    for (index, row) in shown.iter().enumerate() {
        // A row that continues reads to the wrap's width; a line's last, as far as shaped.
        let limit = if row.continues {
            screen.drawn(row).end
        } else {
            row.end_column
        };
        let columns = row.column + shaped.start..row.column.saturating_add(shaped.end).min(limit);
        let mark = screen.mark(row).map(|column| screen.origin(row) + column);
        let spliced = preedit.filter(|_| row.line == view.cursor.row);
        let line = (row.line, columns);
        let shaped_row = rows.view_row(display, line, mark, spliced, system);
        let row_layout = layout.with_left(screen.origin(row));
        if !draws.row(&row_layout, index, shaped_row, system, glyphs) {
            break;
        }
    }
    if let (Some(first), Some(last)) = (shown.first(), shown.last()) {
        rows.evict(
            (first.line, first.column)..=(last.line, last.column),
            shown.len(),
        );
    }
}
