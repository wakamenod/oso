//! A reply's table as it is drawn (ADR 0165, "The look: tables"): which lines are one, what it may
//! take, and laying it out; its cells measured and wrapped are `grid`'s, over `breaks`, and its
//! lines between box rules `boxed`'s.
//!
//! Must not depend on: the syntax threads, the editor or the theme; a pure function of a
//! table's text and a width.
//!
//! - A table is a run of lines that start with `|`, where `shown` reads marks: a header, a line of dashes for as many
//!   columns, then its rows. A row's cells past the last column are part of its last, so no word
//!   of it goes.
//! - Cells are measured as the display draws them, in clusters: Japanese and other wide
//!   characters, and escapes. What would not stand apart from the rules around it, or be drawn as
//!   measured, leaves the table as written, as does a table past a limit.
//! - Only inline marks are read in a cell, and they are matched in it alone. A link in it hides
//!   its target only when the row's other cells hold nothing that could read as one.
//! - O(text + cells drawn).

mod boxed;
mod breaks;
mod grid;

use boxed::Layout;
use breaks::widths;
use grid::Grid;

use super::{BLANK, MAX_MARKED, Style, trimmed};

/// The widest a reply's table is, with what stands before it, in cells (the author's choice).
const MAX_TABLE_CELLS: usize = 100;
/// The cells before a reply's line, and for each depth it is nested at.
const INDENT: usize = 2;
/// The most columns of a table drawn between rules, as many as a reply's width has room for.
const MAX_COLUMNS: usize = 24;
/// The fewest lines of a table between rules: two of them, and its header.
const MIN_LINES: usize = 3;
/// The most lines of one, and one more for each so many bytes of it, so that few bytes do not
/// draw many.
const FEW_LINES: usize = 64;
const BYTES_A_LINE: usize = 4;
/// How wide the tables of a reply at `depth` may be, past its indent, in a window `width` cells
/// wide.
pub fn table_width(depth: u8, width: usize) -> usize {
    (width.min(MAX_TABLE_CELLS)).saturating_sub(INDENT * (usize::from(depth) + 1))
}

/// Where a column's text stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// What a run of `|` lines is so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Found {
    /// No table: its second line is no line of dashes.
    No,
    /// It may go on: the text ends first, and more may come.
    Open,
    /// A table.
    Whole(Whole),
}

/// A table that ends: at byte `end`, of `rows` lines with that of dashes, and `columns`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Whole {
    pub(super) end: usize,
    rows: usize,
    columns: usize,
}

/// What a reply's tables may still take. What a table would is taken before it is drawn, whether
/// or not it then is, so that none costs what it is not charged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Budget {
    /// Their text's bytes, their cells, and the lines they are drawn in.
    bytes: u32,
    cells: u32,
    lines: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            bytes: 64 << 10,
            cells: 16 << 10,
            lines: 2 << 10,
        }
    }
}

impl Budget {
    /// About what was taken of it, in cells.
    pub(super) fn taken(self) -> usize {
        let whole = Self::default();
        let lines = (whole.lines - self.lines) * 8;
        (whole.cells - self.cells + lines + (whole.bytes - self.bytes) / 8) as usize
    }

    /// Takes what `whole`, of `bytes`, has; whether it was there.
    fn take(&mut self, whole: Whole, bytes: usize) -> bool {
        let cells = whole.rows.checked_mul(whole.columns);
        let left = |left: u32, need: usize| left.checked_sub(u32::try_from(need).ok()?);
        let taken =
            cells.and_then(|cells| Some((left(self.bytes, bytes)?, left(self.cells, cells)?)));
        let Some((bytes, cells)) = taken.filter(|_| self.lines as usize >= MIN_LINES) else {
            return false;
        };
        (self.bytes, self.cells) = (bytes, cells);
        true
    }
}

/// How many spaces `line` starts with, if it can be a table's.
pub(super) fn piped(line: &str) -> Option<usize> {
    let body = line.trim_start_matches(' ');
    body.starts_with('|').then_some(line.len() - body.len())
}

/// What the run of `|` lines of `text` from byte `from` on, its header there, is; `streams` if
/// more text may come.
pub(super) fn find(text: &str, from: usize, streams: bool) -> Found {
    let mut at = from;
    let (mut header, mut rows) = (0, 0);
    for whole in text[from..].split_inclusive('\n') {
        let ended = whole.ends_with('\n');
        if !ended && streams {
            return Found::Open;
        }
        let line = trimmed(whole);
        if piped(line).is_none() {
            break;
        }
        match rows {
            0 => header = columns(line),
            1 if dashes(line) != Some(header) => return Found::No,
            _ => {}
        }
        rows += 1;
        at += whole.len();
    }
    match rows {
        _ if at == text.len() && streams => Found::Open,
        0 | 1 => Found::No,
        _ => Found::Whole(Whole {
            end: at,
            rows,
            columns: header,
        }),
    }
}

/// Whether a table goes on to the end of `rest`, what follows its lines so far, in which more may
/// come.
pub(super) fn goes_on(rest: &str) -> bool {
    (rest.split_inclusive('\n')).all(|whole| !whole.ends_with('\n') || piped(whole).is_some())
}

/// The text of a row between its outer pipes.
fn inner(line: &str) -> &str {
    let line = line.trim_matches(BLANK);
    let line = line.strip_prefix('|').unwrap_or(line);
    match line.strip_suffix('|') {
        Some(rest) if !escaped(rest) => rest,
        _ => line,
    }
}

/// Whether what follows `text` is escaped by a backslash.
fn escaped(text: &str) -> bool {
    let slashes = text.len() - text.trim_end_matches('\\').len();
    slashes % 2 == 1
}

/// The cells of a row between its outer pipes, at most `most`, the rest of the row in its last.
fn cells(inner: &str, most: usize) -> impl Iterator<Item = &str> {
    let mut rest = Some(inner);
    let mut left = most;
    std::iter::from_fn(move || {
        let text = rest?;
        left = left.checked_sub(1)?;
        let end = (left > 0).then(|| pipe(text)).flatten();
        rest = end.map(|end| &text[end + 1..]);
        Some(text[..end.unwrap_or(text.len())].trim_matches(BLANK))
    })
}

/// The first pipe of `text` that no backslash escapes.
fn pipe(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'|' => return Some(at),
            _ => at += 1,
        }
    }
    None
}

/// How many cells a row has.
fn columns(line: &str) -> usize {
    cells(inner(line), usize::MAX).count()
}

/// How many columns a line of dashes has.
fn dashes(line: &str) -> Option<usize> {
    let mut columns = 0;
    for cell in cells(inner(line), usize::MAX) {
        align(cell)?;
        columns += 1;
    }
    Some(columns)
}

/// How the column of `cell`, of a line of dashes, stands.
fn align(cell: &str) -> Option<Align> {
    let rest = cell.strip_prefix(':').unwrap_or(cell);
    let dashes = rest.strip_suffix(':').unwrap_or(rest);
    if dashes.is_empty() || dashes.bytes().any(|byte| byte != b'-') {
        return None;
    }
    Some(match (rest.len() < cell.len(), dashes.len() < rest.len()) {
        (true, true) => Align::Center,
        (false, true) => Align::Right,
        _ => Align::Left,
    })
}

/// Lays `text`, the lines of `whole`, out in at most `width` cells, with what it takes of
/// `budget`; `None` for a table that is to be drawn as written.
pub(super) fn lay(text: &str, whole: Whole, width: usize, budget: &mut Budget) -> Option<Layout> {
    let mut lines = text.split_inclusive('\n').map(trimmed);
    let header = lines.next()?;
    let indent = piped(header)?;
    let columns = whole.columns;
    // Its rules and the space beside each cell.
    let room = width.checked_sub(indent + 3 * columns + 1)?;
    if room < columns || columns > MAX_COLUMNS || !budget.take(whole, text.len()) {
        return None;
    }
    let aligns = cells(inner(lines.next()?), columns);
    let aligns = aligns.map(align).collect::<Option<Vec<_>>>()?;
    let mut grid = Grid {
        widest: vec![(0, 0); columns],
        cells: Vec::with_capacity(columns * whole.rows),
        ..Grid::default()
    };
    for (number, line) in std::iter::once(header).chain(lines).enumerate() {
        if line.len() > MAX_MARKED {
            return None;
        }
        grid.read(line, columns, (number == 0).then_some(Style::Strong))?;
    }
    let widths = widths(&grid.widest, room)?;
    let most = (budget.lines as usize).min(FEW_LINES + text.len() / BYTES_A_LINE);
    let laid = grid.wrap(&widths, most);
    // One that fails was wrapped so far.
    let lines = laid.map_or(most, |(lines, _)| lines);
    budget.lines -= u32::try_from(lines).ok()?;
    let (lines, ruled) = laid?;
    Some(Layout {
        grid,
        indent,
        widths,
        aligns,
        ruled,
        lines,
    })
}

#[cfg(test)]
mod tests;
