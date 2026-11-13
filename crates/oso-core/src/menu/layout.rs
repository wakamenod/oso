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
    /// The title's row, short of the prefix argument's cells where one is shown.
    pub title: CellRect,
    /// The prefix argument's cells at the end of the title's row, such as `C-u 5-`, if one is
    /// being typed and the row has room for it.
    pub prefix: Option<CellRect>,
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
    /// Its group, as an index into [`Definition::groups`].
    pub group: usize,
    /// The row of its title, if it has one and the frame leaves room for titles.
    pub title: Option<CellRect>,
    /// The row of the `╌` rule under its title.
    pub rule: Option<CellRect>,
    /// The heads it shows, each on a row of its own: `key  [x] label  note`, the labels and the
    /// notes each starting in one column of their own.
    pub rows: Vec<Row>,
    /// How many of its heads were cut for room, and the row below its last shown head that says
    /// `… N more`, sized for the number. Hidden heads are not counted. `None` where none was cut,
    /// or where even that row has no room.
    pub more: Option<(usize, CellRect)>,
}

/// One shown head's row, and its parts' cells. Each part is cut to its column, and a part with
/// no room left is `None`, or has no columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The head, as an index into [`Definition::heads`].
    pub head: usize,
    /// The whole row.
    pub cells: CellRect,
    /// Its keys, as wide as they are spelled: their chip.
    pub key: CellRect,
    /// The cells of `[x]` or `[ ]`, for a switch or a command with a state.
    pub check: Option<CellRect>,
    /// Its label.
    pub label: CellRect,
    /// Its note, if it has one.
    pub note: Option<CellRect>,
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

/// The most bytes of a note that are measured or drawn, as a label's are capped where it is
/// defined: a describe function is built in, but what it reads may be long.
const MAX_NOTE_BYTES: usize = 512;

/// `text`, cut to at most [`MAX_NOTE_BYTES`] bytes at a character's boundary.
pub(crate) fn capped(text: &str) -> &str {
    if text.len() <= MAX_NOTE_BYTES {
        return text;
    }
    let mut end = MAX_NOTE_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Where `definition`'s float and its parts go, centred in `area`, the tiled area, and fitted to
/// its content: the title, the describe function's line, then its rows of columns, each column
/// with an optional title over a rule and a head per row, then `q quit`. `checks` has, by head,
/// `Some(on)` for a switch or a command with a state, and `None` for any other or a head past
/// its end. Hidden heads are left out, and a group left with none. `prefix` is the cells of the
/// prefix argument being typed, shown at the end of the title's row, or 0 for none.
///
/// A float never comes closer than 2 columns or 1 row to an edge. Where it does not fit, it gives
/// up its padding, then the column titles, then moves columns of a row onto a new row below,
/// then cuts the longest columns, a row at a time and the longest first, whose last row then
/// says how many heads it left out. What is still too wide is cut where it is drawn.
pub fn layout(
    definition: &Definition,
    notes: &Notes,
    checks: &[Option<bool>],
    prefix: usize,
    area: CellRect,
) -> Layout {
    let mut blocks = blocks(definition, notes, checks);
    let room = CellRect {
        row: area.row + EDGE_ROWS.min(area.rows / 2),
        column: area.column + EDGE_COLUMNS.min(area.columns / 2),
        rows: area.rows.saturating_sub(2 * EDGE_ROWS),
        columns: area.columns.saturating_sub(2 * EDGE_COLUMNS),
    };
    let shape = Shape {
        line: notes.line.is_some(),
        quit: definition.quits_on_q(),
        prefix,
    };
    let fits = |fit: &Fit, blocks: &[Block<'_>]| {
        let (columns, rows) = fit.outer_size(blocks, shape);
        columns <= room.columns && rows <= room.rows
    };
    let mut fit = Fit {
        padded: true,
        titled: true,
        lines: by_rows(&blocks),
    };
    for stage in 0..3 {
        if fits(&fit, &blocks) {
            break;
        }
        match stage {
            0 => fit.padded = false,
            1 => fit.titled = false,
            _ => fit.lines = reflowed(&blocks, &fit, room.columns),
        }
    }
    // Cutting a column makes it no narrower, so only rows are cut for.
    while fit.outer_size(&blocks, shape).1 > room.rows {
        // The column that takes the most rows, the first of those that do.
        let mut longest: Option<&mut Block<'_>> = None;
        for block in &mut blocks {
            if longest.as_ref().is_none_or(|l| block.shown() > l.shown()) {
                longest = Some(block);
            }
        }
        match longest {
            Some(block) if block.shown() > 1 => block.limit = block.shown() - 1,
            _ => break,
        }
    }
    place(definition, notes, &blocks, &fit, shape, room)
}

/// What a float has beside its columns.
#[derive(Clone, Copy)]
struct Shape {
    line: bool,
    quit: bool,
    /// The prefix argument's cells.
    prefix: usize,
}

/// One group's column, measured.
struct Block<'d> {
    /// Its group's index in the definition.
    group: usize,
    title: Option<&'d str>,
    title_cells: usize,
    row: u8,
    /// Its shown heads.
    heads: Vec<usize>,
    /// Each shown head's parts, measured.
    measures: Vec<Measure>,
    /// The cells of its widest key.
    keys: usize,
    /// The cells of its widest `[x]` and label.
    labels: usize,
    /// The cells of its widest head's row: the keys, the labels and the notes, each column as
    /// wide as its widest.
    body: usize,
    /// The most rows it takes: with more heads, it shows one fewer, and then `… N more`.
    limit: usize,
}

/// The cells of one head's parts.
struct Measure {
    key: usize,
    /// Whether it has `[x]` or `[ ]` before its label.
    check: bool,
    label: usize,
    note: usize,
}

/// How a float is fitted to the room it has.
struct Fit {
    padded: bool,
    titled: bool,
    /// The rows of columns, each a list of blocks, left to right.
    lines: Vec<Vec<usize>>,
}

impl Block<'_> {
    /// The rows its heads take, `… N more` included.
    fn shown(&self) -> usize {
        self.heads.len().min(self.limit)
    }

    /// How many of its heads are cut: none, or all but `limit - 1`.
    fn cut(&self) -> usize {
        if self.heads.len() > self.limit {
            self.heads.len() - (self.limit - 1)
        } else {
            0
        }
    }

    fn width(&self, fit: &Fit) -> usize {
        let title = if fit.titled { self.title_cells } else { 0 };
        let more = match self.cut() {
            0 => 0,
            cut => more_cells(cut),
        };
        self.body.max(title).max(more)
    }

    fn height(&self, fit: &Fit) -> usize {
        let title = if fit.titled && self.title.is_some() {
            2
        } else {
            0
        };
        title + self.shown()
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

/// The cells `… N more` takes for `cut` heads left out: `…`, a space, the number, and ` more`.
/// Counted, not written, since a layout may measure it many times.
fn more_cells(cut: usize) -> usize {
    let digits = cut.max(1).ilog10() as usize + 1;
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
    for (index, group) in definition.groups().iter().enumerate() {
        let shown: Vec<usize> = group
            .heads()
            .filter(|&i| !by_head[i].is_some_and(|note| note.hidden))
            .collect();
        if shown.is_empty() {
            continue;
        }
        let measures: Vec<Measure> = shown
            .iter()
            .map(|&i| {
                spelled.clear();
                let _ = write!(spelled, "{}", heads[i].keys);
                Measure {
                    key: cells_of(&spelled),
                    check: checks.get(i).copied().flatten().is_some(),
                    label: cells_of(&heads[i].label),
                    note: by_head[i].map_or(0, |note| cells_of(capped(&note.text))),
                }
            })
            .collect();
        let widest = |part: fn(&Measure) -> usize| measures.iter().map(part).max().unwrap_or(0);
        let keys = widest(|m| m.key);
        let labels = widest(|m| if m.check { CHECK } else { 0 } + m.label);
        let notes = widest(|m| m.note);
        let body = keys + GAP + labels + if notes > 0 { GAP + notes } else { 0 };
        let title = group.title();
        blocks.push(Block {
            group: index,
            title,
            title_cells: title.map_or(0, cells_of),
            row: group.row(),
            heads: shown,
            measures,
            keys,
            labels,
            body,
            limit: usize::MAX,
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

mod place;

use place::place;

#[cfg(test)]
mod tests;
