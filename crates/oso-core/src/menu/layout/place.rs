//! Where each part of a menu's float goes once its fit is worked out (ADR 0117): the title and
//! the prefix argument, the describe function's line, the columns and their rows, cut to the
//! room the float has.
//!
//! Must not depend on: `keymap`, or any GPU or window-system type.

use super::{
    Block, CHECK, COLUMN_GAP, Column, Fit, GAP, Layout, QUIT, Row, SIDE_COLUMNS, SIDE_ROWS, Shape,
    capped, more_cells,
};
use crate::frame::CellRect;
use crate::menu::{Definition, Notes};
use crate::minibuffer::cells_of;

/// The cells of each part, for `fit`, centred in `room`.
pub(super) fn place(
    definition: &Definition,
    notes: &Notes,
    blocks: &[Block<'_>],
    fit: &Fit,
    shape: Shape,
    room: CellRect,
) -> Layout {
    let (columns_width, columns_rows) = fit.columns_size(blocks);
    let line_cells = notes
        .line
        .as_deref()
        .map_or(0, |line| cells_of(capped(line)));
    let quit_cells = if shape.quit { cells_of(QUIT) } else { 0 };
    let prefix_cells = if shape.prefix > 0 {
        GAP + shape.prefix
    } else {
        0
    };
    let (side, ends) = if fit.padded {
        (SIDE_COLUMNS, SIDE_ROWS)
    } else {
        (0, 0)
    };
    let wanted = columns_width
        .max(quit_cells)
        .max(cells_of(definition.title()) + prefix_cells)
        .max(line_cells);
    let inner_columns = wanted.min(room.columns.saturating_sub(2 * side));
    let inner_rows = fit
        .inner_rows(columns_rows, shape)
        .min(room.rows.saturating_sub(2 * ends));
    let outer = CellRect {
        row: room.row + room.rows.saturating_sub(inner_rows + 2 * ends) / 2,
        column: room.column + room.columns.saturating_sub(inner_columns + 2 * side) / 2,
        rows: (inner_rows + 2 * ends).min(room.rows),
        columns: (inner_columns + 2 * side).min(room.columns),
    };
    let inner = CellRect {
        row: outer.row + ends.min(outer.rows),
        column: outer.column + side.min(outer.columns),
        rows: inner_rows,
        columns: inner_columns,
    };
    let bottom = inner.row + inner.rows;
    // The quit row is the last, below the title, and the columns end above it.
    let quit_row = (shape.quit && inner.rows > 1).then(|| bottom - 1);
    let place = Place {
        end: inner.column + inner.columns,
        below: quit_row.unwrap_or(bottom),
    };
    let (title, prefix) = title_row(inner, shape.prefix);
    let y = inner.row + 1;
    let line = shape
        .line
        .then(|| place.rect(y, inner.column, inner.columns))
        .flatten();
    let first = y + usize::from(shape.line) + fit.blank();
    let columns = place_columns(blocks, fit, (inner.column, first), &place);
    let quit = quit_row.and_then(|row| {
        Place {
            end: place.end,
            below: bottom,
        }
        .rect(row, inner.column, quit_cells)
    });
    Layout {
        outer,
        inner,
        title,
        prefix,
        line,
        columns,
        quit,
    }
}

/// The title's row in `inner`, and the cells of a prefix argument `prefix` cells wide at its
/// end, where it fits whole; the title is cut short of it.
fn title_row(inner: CellRect, prefix: usize) -> (CellRect, Option<CellRect>) {
    let shown = (prefix > 0 && inner.rows > 0 && prefix <= inner.columns).then(|| CellRect {
        row: inner.row,
        column: inner.column + inner.columns - prefix,
        rows: 1,
        columns: prefix,
    });
    let title = CellRect {
        rows: inner.rows.min(1),
        columns: match shown {
            Some(_) => inner.columns.saturating_sub(GAP + prefix),
            None => inner.columns,
        },
        ..inner
    };
    (title, shown)
}

/// The columns of `fit`'s rows of columns, from `(x, y)` down, a blank row between rows of
/// columns where it is padded.
fn place_columns(
    blocks: &[Block<'_>],
    fit: &Fit,
    (x, mut y): (usize, usize),
    place: &Place,
) -> Vec<Column> {
    let mut columns = Vec::new();
    for (i, line) in fit.lines.iter().enumerate() {
        if i > 0 {
            y += fit.blank();
        }
        let mut at = x;
        let mut height = 0;
        for &b in line {
            let block = &blocks[b];
            let width = block.width(fit);
            columns.push(column(block, fit, (at, y, width), place));
            height = height.max(block.height(fit));
            at += width + COLUMN_GAP;
        }
        y += height;
    }
    columns
}

/// Where rows may go: left of column `end`, above row `below`.
struct Place {
    end: usize,
    below: usize,
}

impl Place {
    /// One row at `row`, from `column`, `columns` wide, cut at the end; `None` where it has no
    /// room at all.
    fn rect(&self, row: usize, column: usize, columns: usize) -> Option<CellRect> {
        (row < self.below && column < self.end).then(|| CellRect {
            row,
            column,
            rows: 1,
            columns: columns.min(self.end - column),
        })
    }

    /// `[column, column + columns)` on `row`, cut to `within`, which may leave no columns.
    fn part(within: CellRect, column: usize, columns: usize) -> CellRect {
        let end = within.column + within.columns;
        let column = column.min(end);
        CellRect {
            column,
            columns: columns.min(end - column),
            ..within
        }
    }
}

/// One block's column at `(x, y)`, `width` cells wide, its rows cut where `place` finds no
/// room.
fn column(
    block: &Block<'_>,
    fit: &Fit,
    (x, y, width): (usize, usize, usize),
    place: &Place,
) -> Column {
    let titled = fit.titled && block.title.is_some();
    let title = if titled {
        place.rect(y, x, width)
    } else {
        None
    };
    let rule = if titled {
        place.rect(y + 1, x, width)
    } else {
        None
    };
    let first = y + if titled { 2 } else { 0 };
    let cut = block.cut();
    let shown = &block.heads[..block.heads.len() - cut];
    let label_at = x + block.keys + GAP;
    let note_at = label_at + block.labels + GAP;
    let rows = shown
        .iter()
        .enumerate()
        .filter_map(|(i, &head)| {
            let cells = place.rect(first + i, x, width)?;
            let measure = &block.measures[i];
            let check = measure.check.then(|| Place::part(cells, label_at, CHECK));
            let label_start = label_at + if measure.check { CHECK } else { 0 };
            Some(Row {
                head,
                cells,
                key: Place::part(cells, x, measure.key),
                check,
                label: Place::part(cells, label_start, measure.label),
                note: (measure.note > 0).then(|| Place::part(cells, note_at, measure.note)),
            })
        })
        .collect();
    let more = match cut {
        0 => None,
        cut => place
            .rect(first + shown.len(), x, more_cells(cut))
            .map(|rect| (cut, rect)),
    };
    Column {
        title,
        rule,
        rows,
        more,
    }
}
