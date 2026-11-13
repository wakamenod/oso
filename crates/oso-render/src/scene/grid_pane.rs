//! What a pane shows of a terminal's grid (ADR 0120): each row's backgrounds, boxes, lines and
//! glyphs in its cells' own colours, a preedit over the cells from the cursor's on, and the
//! cursor.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - A row is read and shaped only when its generation or its columns change (`cells`,
//!   `rows`). Every frame draws what was shaped, its colours resolved in the frame's theme, at
//!   a cost of the cells shown.
//! - Backgrounds, underlines and strikes are filled in runs of one colour, behind the boxes,
//!   which no cell's colour can hide. Glyphs keep their cells' columns, and a glyph paints only
//!   its cells (ADR 0052).
//! - The preedit covers the cells under it in the text's background, and is shaped as text
//!   that is no buffer's, so its own invisible characters show as labels (ADR 0069).

use std::ops::Range;

use oso_core::frame::{CellRect, PixelRect};
use oso_platform::text::{CellMetrics, TextSystem};

use super::{Draws, GlyphSource, line_width, outline};
use crate::cells::{Ink, cells_of, ink, is_blank, read_row};
use crate::grid::PlacedGlyph;
use crate::pane::{Face, GridCell, GridCursorShape, GridStyle, GridView, Pane, Span};
use crate::rows::{Rows, ShapedRow};
use crate::text::Layout;

/// The key the preedit's row is kept by among a pane's rows, past any grid row's.
const PREEDIT_ROW: usize = usize::MAX;

/// Where the cursor is drawn, and how.
struct Cursor {
    /// The frame's cells it takes.
    cells: CellRect,
    shape: GridCursorShape,
    hollow: bool,
}

/// The cursor of `view` in `region`, where a preedit puts it (ADR 0069), if it is in the region.
/// It takes two cells on a wide one.
fn cursor_of(view: &GridView<'_>, region: CellRect) -> Option<Cursor> {
    let cursor = view.cursor?;
    let (row, column) = (usize::from(cursor.row), usize::from(cursor.column));
    let width_at = |column: usize| {
        let cell = view.rows.get(row).and_then(|row| row.cells.get(column));
        cell.map_or(1, cells_of)
    };
    let (column, columns) = match view.preedit.map(|preedit| (preedit.caret(), preedit)) {
        None => (column, width_at(column)),
        Some((Some((before, cells)), _)) => (column.saturating_add(before), cells.max(1)),
        Some((None, preedit)) => {
            let after = column.saturating_add(preedit.cells());
            (after, width_at(after))
        }
    };
    (row < region.rows && column < region.columns).then(|| Cursor {
        cells: CellRect {
            row: region.row + row,
            column: region.column + column,
            rows: 1,
            columns: columns.min(region.columns - column),
        },
        shape: cursor.shape,
        hollow: cursor.hollow,
    })
}

/// Where the preedit of `view` goes in `region`: its row and its columns, cut to the region.
fn preedit_cells(view: &GridView<'_>, region: CellRect) -> Option<(usize, Range<usize>)> {
    let preedit = view.preedit?;
    let (row, column) = (
        usize::from(view.preedit_at.0),
        usize::from(view.preedit_at.1),
    );
    let end = column.saturating_add(preedit.cells()).min(region.columns);
    (row < region.rows && column < end).then_some((row, column..end))
}

/// The rectangles the cursor and the preedit of `view` need, which the cells' cannot take:
/// the cursor's one, or a hollow one's four sides, and the preedit's cover and two underlines.
pub(super) fn held(view: &GridView<'_>, region: CellRect) -> usize {
    let cursor = cursor_of(view, region).map_or(0, |cursor| if cursor.hollow { 4 } else { 1 });
    let preedit = if preedit_cells(view, region).is_some() {
        3
    } else {
        0
    };
    cursor + preedit
}

/// One grid row being drawn: its layout and row, its cells shown, and a thin line's width.
struct RowDraw<'a, 'b> {
    layout: &'a Layout,
    index: usize,
    cells: &'b [GridCell],
    line: f32,
    /// The frame's cells under the cursor box, whose glyphs keep the box's colours.
    cursor: Option<CellRect>,
}

impl RowDraw<'_, '_> {
    /// Whether any of `cells` cells from `column` is under the cursor box.
    fn under_cursor(&self, column: usize, cells: u32) -> bool {
        let [row, column] = self.layout.frame_cell(self.index, column);
        self.cursor.is_some_and(|box_cells| {
            let (top, left) = (box_cells.row as i64, box_cells.column as i64);
            row == top
                && column < left + box_cells.columns as i64
                && left < column + i64::from(cells)
        })
    }

    /// The pixels of `columns` of the row, cut to the pane.
    fn cells_at(&self, columns: Range<usize>) -> Option<PixelRect> {
        (self.layout).display_cells(self.index, columns.start, columns.len())
    }

    /// A line `height` thick, `up` pixels above the bottom of `columns`.
    fn line_at(&self, columns: Range<usize>, up: f32, height: f32) -> Option<PixelRect> {
        self.cells_at(columns).map(|rect| PixelRect {
            y: rect.y + rect.height - up - height,
            height,
            ..rect
        })
    }
}

impl Draws {
    /// Fills, in runs, the cells of `row` that are `wanted`, each in the colour `color` gives
    /// it, in the rectangles `rect` gives the runs. A cell's colours are resolved only when it
    /// is wanted.
    fn fill_runs(
        &mut self,
        row: &RowDraw<'_, '_>,
        wanted: impl Fn(&GridCell) -> bool,
        color: impl Fn(&Ink) -> Option<[f32; 4]>,
        rect: impl Fn(&RowDraw<'_, '_>, Range<usize>) -> Option<PixelRect>,
    ) {
        let mut run: Option<(usize, [f32; 4])> = None;
        for (column, cell) in row.cells.iter().enumerate() {
            let next = (wanted(cell))
                .then(|| color(&ink(cell, &self.theme, &self.decode)))
                .flatten();
            if run.is_some_and(|(_, open)| Some(open) == next) {
                continue;
            }
            if let Some((start, open)) = run.take() {
                self.fill_color(rect(row, start..column), open);
            }
            run = next.map(|color| (column, color));
        }
        if let Some((start, open)) = run {
            self.fill_color(rect(row, start..row.cells.len()), open);
        }
    }

    /// Fills what `row` draws behind its glyphs: its backgrounds, its boxes, and its lines.
    fn fill_grid_row(&mut self, row: &RowDraw<'_, '_>, shaped: &ShapedRow) {
        let has = |flag: GridStyle| move |cell: &GridCell| cell.style.contains(flag);
        let filled = |cell: &GridCell| {
            cell.bg != crate::pane::GridColor::Default || cell.style.contains(GridStyle::INVERSE)
        };
        self.fill_runs(
            row,
            filled,
            |ink| ink.bg,
            |row, columns| row.cells_at(columns),
        );
        for (i, &(column, cells)) in shaped.boxes.iter().enumerate() {
            // A box followed by another ends a pixel short, so two never read as one.
            let joined = (shaped.boxes)
                .get(i + 1)
                .is_some_and(|&(next, _)| next == column + cells);
            let rect = row.cells_at(column..column + cells).map(|rect| PixelRect {
                width: rect.width - if joined { 1.0 } else { 0.0 },
                ..rect
            });
            self.fill(rect.filter(|rect| rect.width > 0.0), Face::Boxed);
        }
        let line = row.line;
        let under = |ink: &Ink| Some(ink.underline);
        let single = has(GridStyle::UNDERLINE);
        self.fill_runs(row, single, under, |row, c| row.line_at(c, 0.0, line));
        let double = has(GridStyle::DOUBLE_UNDERLINE);
        self.fill_runs(row, double, under, |row, c| row.line_at(c, 0.0, line));
        self.fill_runs(row, double, under, |row, c| {
            row.line_at(c, 2.0 * line, line)
        });
        let strike = has(GridStyle::STRIKE);
        let through = |row: &RowDraw<'_, '_>, columns| {
            row.cells_at(columns).map(|rect| PixelRect {
                y: (rect.y + (rect.height - line) / 2.0).round(),
                height: line,
                ..rect
            })
        };
        self.fill_runs(row, strike, |ink| Some(ink.fg), through);
    }

    /// Adds the glyphs of `row`, as `shaped`, in their cells' colours, but those `covered`
    /// even in part. Returns false once there is no room for more.
    fn grid_glyphs<G: GlyphSource>(
        &mut self,
        row: &RowDraw<'_, '_>,
        (shaped, covered): (&ShapedRow, Option<Range<usize>>),
        system: &mut dyn TextSystem,
        glyphs: &mut G,
    ) -> bool {
        for glyph in &shaped.glyphs {
            let Some(cell) = row.cells.get(glyph.column) else {
                break;
            };
            if !self.room_for_quad() {
                return false;
            }
            let end = glyph.column + glyph.cells as usize;
            let hidden = cell.style.contains(GridStyle::HIDDEN)
                || is_blank(cell)
                || (covered.as_ref()).is_some_and(|c| glyph.column < c.end && c.start < end);
            if hidden {
                continue;
            }
            let Some(entry) = glyphs.get(system, glyph.key) else {
                continue;
            };
            let Some(quad) = (row.layout).quad(row.index, glyph, &entry, &self.theme) else {
                continue;
            };
            // A box keeps its own colours whatever the cell's, and so does the cursor box.
            if glyph.face == Face::Boxed || row.under_cursor(glyph.column, glyph.cells) {
                self.quads.push(quad);
            } else {
                let ink = ink(cell, &self.theme, &self.decode);
                self.quads.push(quad.inked(ink.fg, ink.paper(&self.theme)));
            }
        }
        true
    }
}

/// Draws `view` in `pane`, from its `unscrolled` layout: its rows, then the rectangles `held`
/// kept room for, the preedit and the cursor.
pub(super) fn draw_grid<G: GlyphSource>(
    draws: &mut Draws,
    view: &GridView<'_>,
    pane: &Pane<'_>,
    (unscrolled, held, metrics): (&Layout, usize, CellMetrics),
    (rows, system, glyphs): (&mut Rows, &mut dyn TextSystem, &mut G),
) {
    let region = pane.region;
    let layout = &unscrolled.with_bands(pane.bands);
    // What no row covers, and a cell of the default background, shows the text's.
    if pane.background != Some(Face::Text) {
        draws.fill(layout.frame_cells(region), Face::Text);
    }
    let cursor = cursor_of(view, region);
    let block = cursor
        .as_ref()
        .filter(|cursor| cursor.shape == GridCursorShape::Block && !cursor.hollow);
    // Glyphs take the box's colour only where the box is.
    let layout = block.map_or(*layout, |cursor| layout.with_cursor(cursor.cells));
    let preedit = preedit_cells(view, region);
    rows.begin_grid(view.id);
    let shown = &view.rows[..view.rows.len().min(region.rows)];
    for (index, row) in shown.iter().enumerate() {
        let columns = row.cells.len().min(region.columns);
        let shaped = rows.grid_row(
            index,
            (row.generation, columns),
            |input| read_row(row, columns, input),
            system,
        );
        let draw = RowDraw {
            layout: &layout,
            index,
            cells: &row.cells[..columns],
            line: line_width(metrics),
            cursor: block.map(|cursor| cursor.cells),
        };
        draws.fill_grid_row(&draw, shaped);
        let covered = (preedit.clone()).and_then(|(at, columns)| (at == index).then_some(columns));
        if !draws.grid_glyphs(&draw, (shaped, covered), system, glyphs) {
            break;
        }
    }
    rows.evict(shown.len(), 0..=shown.len());
    draws.held -= held;
    if let Some((row, columns)) = preedit {
        let line = line_width(metrics);
        draw_preedit(
            draws,
            view,
            (&layout, line),
            (row, columns),
            (rows, system, glyphs),
        );
    }
    if let Some(cursor) = cursor {
        draw_cursor(draws, &layout, &cursor, line_width(metrics));
    }
}

/// Draws the preedit of `view` over `columns` of `row`, which it covers in the text's
/// background, and underlines it, one line thick, and the segment being converted two lines
/// thick, as a text view does (ADR 0069).
fn draw_preedit<G: GlyphSource>(
    draws: &mut Draws,
    view: &GridView<'_>,
    (layout, line): (&Layout, f32),
    (row, columns): (usize, Range<usize>),
    (rows, system, glyphs): (&mut Rows, &mut dyn TextSystem, &mut G),
) {
    let Some(preedit) = view.preedit else {
        return;
    };
    let at = columns.start;
    draws.fill(layout.display_cells(row, at, columns.len()), Face::Text);
    let spans = [Span {
        text: preedit.text(),
        face: Face::Preedit,
    }];
    let room = layout.columns().end.saturating_sub(at);
    let shaped = rows.line(PREEDIT_ROW, &spans, room, system);
    for &(column, cells) in &shaped.boxes {
        draws.fill(layout.display_cells(row, at + column, cells), Face::Boxed);
    }
    for glyph in &shaped.glyphs {
        if !draws.room_for_quad() {
            break;
        }
        let Some(entry) = glyphs.get(system, glyph.key) else {
            continue;
        };
        let glyph = PlacedGlyph {
            column: at + glyph.column,
            ..*glyph
        };
        draws
            .quads
            .extend(layout.quad(row, &glyph, &entry, &draws.theme));
    }
    let (all, segment) = preedit.underlines(at);
    let mut underline = |under: Range<usize>, thickness: f32| {
        let (start, end) = (under.start, under.end.min(columns.end));
        if start < end {
            let rect = layout.display_cells(row, start, end - start);
            let rect = rect.map(|rect| PixelRect {
                y: rect.y + rect.height - thickness,
                height: thickness,
                ..rect
            });
            draws.fill(rect, Face::Preedit);
        }
    };
    underline(all, line);
    if let Some(segment) = segment {
        underline(segment, 2.0 * line);
    }
}

/// Draws `cursor`: the cursor face's box, a bar at its left or a line at its bottom, each two
/// lines thick, or a hollow one's outline, as a window that is not selected draws its cursor.
fn draw_cursor(draws: &mut Draws, layout: &Layout, cursor: &Cursor, line: f32) {
    let Some(rect) = layout.frame_cells(cursor.cells) else {
        return;
    };
    if cursor.hollow {
        for side in outline(rect, line) {
            draws.fill(Some(side), Face::Cursor);
        }
        return;
    }
    let thick = 2.0 * line;
    let rect = match cursor.shape {
        GridCursorShape::Block => rect,
        GridCursorShape::Bar => PixelRect {
            width: thick.min(rect.width),
            ..rect
        },
        GridCursorShape::Underline => PixelRect {
            y: rect.y + rect.height - thick.min(rect.height),
            height: thick.min(rect.height),
            ..rect
        },
    };
    draws.fill(Some(rect), Face::Cursor);
}

#[cfg(test)]
mod tests;
