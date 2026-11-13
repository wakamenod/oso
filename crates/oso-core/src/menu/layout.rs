//! Where an open menu's float and its parts go over the tiled area (ADR 0117, Q6, Q9): a pure
//! function of the definition, its notes, the toggle and switch states and the area, as
//! `frame::float_regions` is for prompts.
//!
//! Must not depend on: `keymap`, or any GPU or window-system type. It reads no editor state: the
//! caller reads the states and the notes, and this only counts cells.

use std::fmt::Write as _;

use super::{Definition, Note, Notes};
use crate::frame::CellRect;
use crate::minibuffer::cells_of;

/// Where an open menu's float and its parts go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// All of it: the cells its fill covers, padding included. The ring goes around it.
    pub outer: CellRect,
    /// What it shows, inside the padding; the same as `outer` where no padding fits.
    pub inner: CellRect,
    /// The title's row.
    pub title: CellRect,
    /// The row under the title for a describe function's line, if it gave one.
    pub line: Option<CellRect>,
    /// The columns, row of columns after row of columns, each left to right.
    pub columns: Vec<Column>,
    /// The row that says `q quit`, unless a head binds `q`.
    pub quit: Option<CellRect>,
}

/// One group's column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The row of its title, if it has one and the frame leaves room for titles. Its `╌` rule is
    /// the row below.
    pub title: Option<CellRect>,
    /// The cells its keys take, before the 2 that part them from the labels.
    pub keys: usize,
    /// The heads it shows, each with its row: `key  [x] label  note`.
    pub rows: Vec<(usize, CellRect)>,
    /// How many of its heads were cut for room: the row below its last shown head then reads
    /// `… N more`. Hidden heads are not counted.
    pub more: usize,
}

/// The columns and rows a float keeps from each edge of the area.
const EDGE_COLUMNS: usize = 2;
const EDGE_ROWS: usize = 1;

/// The columns of padding on each side, and the rows above and below, while it fits.
const SIDE_COLUMNS: usize = 2;
const SIDE_ROWS: usize = 1;

/// The cells between a key and its label, and between a label and its note.
const GAP: usize = 2;

/// The cells between two columns.
const COLUMN_GAP: usize = 3;

/// The cells `[x] ` takes before a toggle's or a switch's label.
const CHECK: usize = 4;

/// What the last row says where `q` quits.
const QUIT: &str = "q quit";

/// Where `definition`'s float and its parts go, centred in `area`, the tiled area, and fitted to
/// its content: the title, the describe function's line, then its rows of columns, each column
/// with an optional title over a rule and a head per row, then `q quit`. `checks` has, by head,
/// `Some(on)` for a switch or a command with a state, and `None` for any other or a head past
/// its end. Hidden heads are left out, and a group left with none.
///
/// A float never comes closer than 2 columns or 1 row to an edge. Where it does not fit, it gives
/// up its padding, then the column titles, then moves columns of a row onto a new row below,
/// then cuts the longest columns, whose last row then says how many heads it left out. What is
/// still too wide is cut where it is drawn.
pub fn layout(
    definition: &Definition,
    notes: &Notes,
    checks: &[Option<bool>],
    area: CellRect,
) -> Layout {
    let blocks = blocks(definition, notes, checks);
    let room = CellRect {
        row: area.row + EDGE_ROWS.min(area.rows / 2),
        column: area.column + EDGE_COLUMNS.min(area.columns / 2),
        rows: area.rows.saturating_sub(2 * EDGE_ROWS),
        columns: area.columns.saturating_sub(2 * EDGE_COLUMNS),
    };
    let shape = Shape {
        line: notes.line.is_some(),
        quit: definition.quits_on_q(),
    };
    let longest = blocks
        .iter()
        .map(|block| block.heads.len())
        .max()
        .unwrap_or(0);
    let fits = |fit: &Fit| {
        let (columns, rows) = fit.outer_size(&blocks, shape);
        columns <= room.columns && rows <= room.rows
    };
    let mut fit = Fit {
        padded: true,
        titled: true,
        lines: by_rows(&blocks),
        limit: usize::MAX,
    };
    for stage in 0..3 {
        if fits(&fit) {
            break;
        }
        match stage {
            0 => fit.padded = false,
            1 => fit.titled = false,
            _ => fit.lines = reflowed(&blocks, &fit, room.columns),
        }
    }
    let mut limit = longest;
    while !fits(&fit) && limit > 1 {
        limit -= 1;
        fit.limit = limit;
    }
    place(definition, notes, &blocks, &fit, shape, room)
}

/// What a float has beside its columns.
#[derive(Clone, Copy)]
struct Shape {
    line: bool,
    quit: bool,
}

/// One group's column, measured.
struct Block<'d> {
    title: Option<&'d str>,
    title_cells: usize,
    row: u8,
    /// Its shown heads.
    heads: Vec<usize>,
    /// The cells of its widest key.
    keys: usize,
    /// The cells of its widest head's row.
    body: usize,
}

/// How a float is fitted to the room it has.
struct Fit {
    padded: bool,
    titled: bool,
    /// The rows of columns, each a list of blocks, left to right.
    lines: Vec<Vec<usize>>,
    /// The most rows a column takes: one with more heads shows one fewer, and then `… N more`.
    limit: usize,
}

impl Block<'_> {
    fn width(&self, fit: &Fit) -> usize {
        let title = if fit.titled { self.title_cells } else { 0 };
        let more = if self.heads.len() > fit.limit {
            more_cells(self.heads.len())
        } else {
            0
        };
        self.body.max(title).max(more)
    }

    fn height(&self, fit: &Fit) -> usize {
        let title = if fit.titled && self.title.is_some() {
            2
        } else {
            0
        };
        title + self.heads.len().min(fit.limit)
    }
}

impl Fit {
    /// The columns and rows of what the columns take.
    fn columns_size(&self, blocks: &[Block<'_>]) -> (usize, usize) {
        let mut size = (0, 0);
        for (i, line) in self.lines.iter().enumerate() {
            let width: usize = line.iter().map(|&b| blocks[b].width(self)).sum::<usize>()
                + COLUMN_GAP * line.len().saturating_sub(1);
            let height = line
                .iter()
                .map(|&b| blocks[b].height(self))
                .max()
                .unwrap_or(0);
            size.0 = size.0.max(width);
            size.1 += height + if i > 0 { self.blank() } else { 0 };
        }
        size
    }

    /// The rows of padding between the parts.
    fn blank(&self) -> usize {
        usize::from(self.padded)
    }

    /// The columns and rows of the whole float, but the title and line, which are cut to fit.
    fn outer_size(&self, blocks: &[Block<'_>], shape: Shape) -> (usize, usize) {
        let (columns, rows) = self.columns_size(blocks);
        let quit = if shape.quit { cells_of(QUIT) } else { 0 };
        let (side, ends) = if self.padded {
            (2 * SIDE_COLUMNS, 2 * SIDE_ROWS)
        } else {
            (0, 0)
        };
        (
            columns.max(quit) + side,
            ends + self.inner_rows(rows, shape),
        )
    }

    /// The rows inside the padding, for columns of `rows` rows.
    fn inner_rows(&self, rows: usize, shape: Shape) -> usize {
        let columns = if rows > 0 { self.blank() + rows } else { 0 };
        let quit = if shape.quit { self.blank() + 1 } else { 0 };
        1 + usize::from(shape.line) + columns + quit
    }
}

/// The cells `… N more` may take, for a column of `heads` heads cut to show one fewer than its
/// limit: `…`, a space, the number left out, which is at most `heads - 1`, and ` more`. Counted,
/// not written, since a layout may measure it many times.
fn more_cells(heads: usize) -> usize {
    let digits = heads.saturating_sub(1).max(1).ilog10() as usize + 1;
    2 + digits + 5
}

/// Each group's column, with the heads it shows, measured.
fn blocks<'d>(
    definition: &'d Definition,
    notes: &Notes,
    checks: &[Option<bool>],
) -> Vec<Block<'d>> {
    let heads = definition.heads();
    // Each head's note, found once: a describe function gives at most one per head.
    let mut by_head: Vec<Option<&Note>> = vec![None; heads.len()];
    for (head, note) in notes.heads.iter().rev() {
        if let Some(slot) = by_head.get_mut(*head) {
            *slot = Some(note);
        }
    }
    let mut spelled = String::new();
    let mut blocks = Vec::with_capacity(definition.groups().len());
    for group in definition.groups() {
        let shown: Vec<usize> = group
            .heads()
            .filter(|&i| !by_head[i].is_some_and(|note| note.hidden))
            .collect();
        if shown.is_empty() {
            continue;
        }
        let keys = shown
            .iter()
            .map(|&i| {
                spelled.clear();
                let _ = write!(spelled, "{}", heads[i].keys);
                cells_of(&spelled)
            })
            .max()
            .unwrap_or(0);
        let body = shown
            .iter()
            .map(|&i| {
                let check = if checks.get(i).copied().flatten().is_some() {
                    CHECK
                } else {
                    0
                };
                let note = by_head[i].map_or(0, |note| cells_of(&note.text));
                let note = if note > 0 { GAP + note } else { 0 };
                keys + GAP + check + cells_of(&heads[i].label) + note
            })
            .max()
            .unwrap_or(0);
        let title = group.title();
        blocks.push(Block {
            title,
            title_cells: title.map_or(0, cells_of),
            row: group.row(),
            heads: shown,
            keys,
            body,
        });
    }
    blocks
}

/// The blocks by the row their groups name, each row's in their order.
fn by_rows(blocks: &[Block<'_>]) -> Vec<Vec<usize>> {
    let mut rows: Vec<u8> = blocks.iter().map(|block| block.row).collect();
    rows.sort_unstable();
    rows.dedup();
    rows.iter()
        .map(|&row| {
            (0..blocks.len())
                .filter(|&b| blocks[b].row == row)
                .collect()
        })
        .collect()
}

/// `fit`'s rows with each one's columns moved onto new rows below where they would pass
/// `columns` cells: as many on each as fit, and at least one.
fn reflowed(blocks: &[Block<'_>], fit: &Fit, columns: usize) -> Vec<Vec<usize>> {
    let mut lines = Vec::new();
    for line in &fit.lines {
        let mut current: Vec<usize> = Vec::new();
        let mut width = 0;
        for &b in line {
            let w = blocks[b].width(fit);
            let wider = width
                + if current.is_empty() {
                    w
                } else {
                    COLUMN_GAP + w
                };
            if !current.is_empty() && wider > columns {
                lines.push(std::mem::take(&mut current));
                width = w;
            } else {
                width = wider;
            }
            current.push(b);
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}

/// The cells of each part, for `fit`, centred in `room`.
fn place(
    definition: &Definition,
    notes: &Notes,
    blocks: &[Block<'_>],
    fit: &Fit,
    shape: Shape,
    room: CellRect,
) -> Layout {
    let (columns_width, columns_rows) = fit.columns_size(blocks);
    let text = |text: &str| cells_of(text);
    let line_cells = notes.line.as_deref().map_or(0, text);
    let quit_cells = if shape.quit { cells_of(QUIT) } else { 0 };
    let (side, ends) = if fit.padded {
        (SIDE_COLUMNS, SIDE_ROWS)
    } else {
        (0, 0)
    };
    let wanted = columns_width
        .max(quit_cells)
        .max(text(definition.title()))
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
    let rect_at = |row: usize, column: usize, columns: usize, below: usize| {
        let end = inner.column + inner.columns;
        (row < below && column < end).then(|| CellRect {
            row,
            column,
            rows: 1,
            columns: columns.min(end - column),
        })
    };
    let row_at = |row, column, columns| rect_at(row, column, columns, quit_row.unwrap_or(bottom));
    let title = CellRect {
        rows: inner.rows.min(1),
        ..inner
    };
    let mut y = inner.row + 1;
    let line = shape
        .line
        .then(|| row_at(y, inner.column, inner.columns))
        .flatten();
    y += usize::from(shape.line) + fit.blank();
    let mut columns = Vec::new();
    for (i, line) in fit.lines.iter().enumerate() {
        if i > 0 {
            y += fit.blank();
        }
        let mut x = inner.column;
        let mut height = 0;
        for &b in line {
            let block = &blocks[b];
            let width = block.width(fit);
            columns.push(column(block, fit, x, y, width, &row_at));
            height = height.max(block.height(fit));
            x += width + COLUMN_GAP;
        }
        y += height;
    }
    let quit = quit_row.and_then(|row| rect_at(row, inner.column, quit_cells, bottom));
    Layout {
        outer,
        inner,
        title,
        line,
        columns,
        quit,
    }
}

/// One block's column at row `y` and column `x`, `width` cells wide, its rows cut where
/// `row_at` finds no room.
fn column(
    block: &Block<'_>,
    fit: &Fit,
    x: usize,
    y: usize,
    width: usize,
    row_at: &impl Fn(usize, usize, usize) -> Option<CellRect>,
) -> Column {
    let titled = fit.titled && block.title.is_some();
    let title = if titled { row_at(y, x, width) } else { None };
    let first = y + if titled { 2 } else { 0 };
    let cut = block.heads.len() > fit.limit;
    let shown = if cut {
        fit.limit - 1
    } else {
        block.heads.len()
    };
    let rows = block.heads[..shown]
        .iter()
        .enumerate()
        .filter_map(|(i, &head)| Some((head, row_at(first + i, x, width)?)))
        .collect();
    Column {
        title,
        keys: block.keys,
        rows,
        more: if cut { block.heads.len() - shown } else { 0 },
    }
}

#[cfg(test)]
mod tests;
