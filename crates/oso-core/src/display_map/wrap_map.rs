//! Soft wrap (ADR 0100): a window's view rows over its buffer's logical lines, at the
//! window's width, kept in step with each edit.
//!
//! Must not depend on: `selection`, `buffer`, `window` or other editor state, fonts or
//! the GPU. It reads a [`DisplaySnapshot`] and says where view rows start.
//!
//! - A [`WrapSnapshot`] is a [`SumTree`] of the rows' lengths in bounded items: runs of
//!   logical lines that fit in a row, and at most [`ROWS_PER_ITEM`] rows of one line or
//!   several, which a line that wraps starts (`builder.rs`). Its summary counts bytes, view
//!   rows and logical lines, so a view row and an offset convert both ways in O(log n).
//! - A row ends where `breaks.rs` says, at the width less the mark's column. Below
//!   [`MIN_COLUMNS`] nothing wraps.
//! - An edit rewraps from the row before it, until a row starts where an old one did, past
//!   the edit, at the same column modulo the tab width when a tab follows on the line. From
//!   there every row is as before (`edit.rs`).
//! - A command's rewraps read at most a budget; past it, rows are provisional and stale, and
//!   a [`WrapJob`] rewraps them, or a new width, off the UI thread (`job.rs`, ADR 0102).
//! - In a native session's transcript, the rows a line goes on in are indented, as `indent.rs`
//!   says, by what the line starts with, and hold that many cells fewer; it wraps from more
//!   columns (`fit.rs`, ADR 0165).
//! - Until a job reports, the rows near a window's view are wrapped from an anchor, and the
//!   rest count one row a logical line (`realize.rs`, ADR 0103). A window holds them in a
//!   `WrapMap` (`map.rs`).
//!
//! No `Debug` here prints an offset, as where rows break tells how long words are in a
//! buffer that may hold secrets.

mod ahead;
mod breaks;
mod builder;
mod edit;
mod fit;
mod indent;
mod job;
mod map;
mod realize;
mod spliced;
mod stream;

use std::fmt;
use std::sync::Arc;

use self::builder::Builder;
use self::fit::Fit;
pub(crate) use self::job::SPLICES_READ;
pub use self::job::{COMMAND_BUDGET, Splice, WrapJob, WrapReport};
pub(crate) use self::map::WrapMap;
pub(crate) use self::spliced::SplicedRow;
use self::stream::RowStream;
use super::{DisplayPoint, DisplaySnapshot};
use crate::sum_tree::{Bias, Dimension, Item, SumTree, Summary};

/// Below this many text columns, a window does not wrap and shows no mark, nor does one on a
/// native session's transcript below some more (`fit.rs`). A row then holds
/// the widest display cluster, a boxed label of 8 cells, with the mark's column after it
/// (ADR 0104).
pub const MIN_COLUMNS: usize = 10;

/// The most rows an item of a wrapped line holds, so that an edit copies only a few.
#[cfg(not(test))]
const ROWS_PER_ITEM: usize = 64;
/// Small in tests, whose texts are small too, so that their lines span several items.
#[cfg(test)]
const ROWS_PER_ITEM: usize = 3;

/// How many rows a job wraps between two checks of whether it was superseded.
const STOP_ROWS: usize = 4096;

/// The most logical lines an item of lines that fit holds.
#[cfg(not(test))]
const LINES_PER_ITEM: u32 = 256;
/// Small in tests, as [`ROWS_PER_ITEM`] is.
#[cfg(test)]
const LINES_PER_ITEM: u32 = 2;

/// One view row in an item of rows: its length in bytes, and whether its line goes on and
/// it holds a tab.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Row {
    len: u32,
    /// Whether its logical line goes on in the next row.
    continues: bool,
    /// Whether it holds a tab, whose cells depend on its column modulo the tab width.
    tabs: bool,
}

/// An item of the tree.
#[derive(Clone)]
enum WrapItem {
    /// Whole logical lines, each in one row.
    Lines { lines: u32, bytes: usize },
    /// Consecutive rows, of one logical line or several.
    Rows(Arc<[Row]>),
}

/// What an item, or a subtree, holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WrapSummary {
    bytes: usize,
    rows: usize,
    /// The logical lines that end in it.
    lines: usize,
    /// Whether a row of a line that wraps holds a tab.
    tabs: bool,
}

impl Summary for WrapSummary {
    fn add_summary(&mut self, other: &Self) {
        self.bytes += other.bytes;
        self.rows += other.rows;
        self.lines += other.lines;
        self.tabs |= other.tabs;
    }
}

impl Item for WrapItem {
    type Summary = WrapSummary;

    fn summary(&self) -> WrapSummary {
        match self {
            Self::Lines { lines, bytes } => WrapSummary {
                bytes: *bytes,
                rows: *lines as usize,
                lines: *lines as usize,
                tabs: false,
            },
            Self::Rows(rows) => WrapSummary {
                bytes: rows.iter().map(|row| row.len as usize).sum(),
                rows: rows.len(),
                lines: rows.iter().filter(|row| !row.continues).count(),
                tabs: rows.iter().any(|row| row.tabs),
            },
        }
    }
}

/// A byte offset, as a dimension of the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Bytes(usize);

impl Dimension<WrapSummary> for Bytes {
    fn add_summary(&mut self, summary: &WrapSummary) {
        self.0 += summary.bytes;
    }
}

/// A view row, as a dimension of the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct ViewRows(usize);

impl Dimension<WrapSummary> for ViewRows {
    fn add_summary(&mut self, summary: &WrapSummary) {
        self.0 += summary.rows;
    }
}

/// Where an item starts: its offset, its first view row and the logical lines before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Start {
    bytes: usize,
    rows: usize,
    lines: usize,
}

impl Dimension<WrapSummary> for Start {
    fn add_summary(&mut self, summary: &WrapSummary) {
        self.bytes += summary.bytes;
        self.rows += summary.rows;
        self.lines += summary.lines;
    }
}

/// One row on the screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ViewRow {
    /// The offset where it starts.
    pub start: usize,
    /// The offset where the next row starts, past a line break that ends this one.
    pub end: usize,
    /// Its logical line, as a [`DisplayPoint`]'s row.
    pub line: usize,
    /// The cell column on its logical line where it starts.
    pub column: usize,
    /// Whether its logical line goes on in the next row, so it draws the mark.
    pub continues: bool,
}

impl fmt::Debug for ViewRow {
    /// Leaves out the offsets and columns (see the module docs).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewRow")
            .field("continues", &self.continues)
            .finish_non_exhaustive()
    }
}

/// The view rows of one buffer at one width. O(1) to clone, and it can go to another
/// thread.
#[derive(Clone)]
pub struct WrapSnapshot {
    tree: SumTree<WrapItem>,
    fit: Fit,
}

impl fmt::Debug for WrapSnapshot {
    /// Leaves out the rows (see the module docs).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrapSnapshot")
            .field("rows", &self.row_count())
            .finish_non_exhaustive()
    }
}

/// The offset of the buffer's end.
fn buffer_end(display: &DisplaySnapshot) -> usize {
    display.len()
}

/// Where logical line `line` ends, past its line break: the next line's start, or the
/// buffer's end.
fn line_end(display: &DisplaySnapshot, line: usize, end: usize) -> usize {
    if line + 1 < display.row_count() {
        display.display_point_to_offset(DisplayPoint::new(line + 1, 0), Bias::Left)
    } else {
        end
    }
}

impl WrapSnapshot {
    /// The view rows of `display` in a window `columns` cells wide: each row holds one
    /// column less, for the mark. O(n), so a large buffer is wrapped off the UI thread.
    pub fn new(display: &DisplaySnapshot, columns: usize) -> Self {
        Self::with_fit(display, Fit::new(columns, false))
    }

    /// [`WrapSnapshot::new`] of a buffer whose rows are indented (`indent.rs`).
    #[cfg(test)]
    pub(crate) fn indenting(display: &DisplaySnapshot, columns: usize) -> Self {
        Self::with_fit(display, Fit::new(columns, true))
    }

    /// The view rows of `display` with nothing wrapped: one per logical line.
    pub fn unwrapped(display: &DisplaySnapshot) -> Self {
        Self::with_fit(display, Fit::UNWRAPPED)
    }

    fn with_fit(display: &DisplaySnapshot, fit: Fit) -> Self {
        let never = || false;
        Self::with_fit_unless(display, fit, &never).unwrap_or_else(|| Self {
            tree: SumTree::new(),
            fit,
        })
    }

    /// [`WrapSnapshot::with_fit`], or `None` once `stop` says so, which it asks every
    /// [`STOP_ROWS`] rows.
    fn with_fit_unless(
        display: &DisplaySnapshot,
        fit: Fit,
        stop: &dyn Fn() -> bool,
    ) -> Option<Self> {
        let mut builder = Builder::new(true);
        for (index, row) in RowStream::new(display, 0, fit).enumerate() {
            if index % STOP_ROWS == STOP_ROWS - 1 && stop() {
                return None;
            }
            builder.row(row);
        }
        Some(Self {
            tree: builder.finish().into_iter().collect(),
            fit,
        })
    }

    /// The rows of `display` at `columns`, every line's clusters read, as no line is found to
    /// fit from its bytes.
    #[cfg(test)]
    fn scanned(display: &DisplaySnapshot, columns: usize) -> Self {
        let fit = Fit::new(columns, false);
        let mut builder = Builder::new(true);
        let mut stream = RowStream::new(display, 0, fit);
        stream.plain = false;
        if stream.whole.is_some() {
            stream.whole = None;
            stream.open(DisplayPoint::new(stream.line, 0));
        }
        for row in stream {
            builder.row(row);
        }
        Self {
            tree: builder.finish().into_iter().collect(),
            fit,
        }
    }

    /// About how many bytes of memory its rows take, its items and their summaries, for
    /// measuring (ADR 0100).
    pub fn heap_bytes(&self) -> usize {
        let item = std::mem::size_of::<WrapItem>() + std::mem::size_of::<WrapSummary>();
        self.tree
            .iter()
            .map(|entry| match entry {
                WrapItem::Lines { .. } => item,
                // An `Arc<[Row]>` has two counts before its rows.
                WrapItem::Rows(rows) => item + 16 + rows.len() * std::mem::size_of::<Row>(),
            })
            .sum()
    }

    /// How many bytes its rows cover, which is its buffer's length.
    pub(crate) fn len(&self) -> usize {
        self.tree.summary().bytes
    }

    /// The cells a row's text takes, its indent among them, the mark's column after them; `usize::MAX` when nothing
    /// wraps.
    pub(crate) fn width(&self) -> usize {
        self.fit.width
    }

    /// The cells the rows that logical line `line` goes on in are indented by: 0 unless its
    /// buffer's rows are (`indent.rs`).
    pub(crate) fn indent(&self, display: &DisplaySnapshot, line: usize) -> usize {
        self.fit.indent(display, line)
    }

    /// Whether rows wrap at all.
    pub fn wraps(&self) -> bool {
        self.fit.wraps()
    }

    /// How many view rows there are.
    pub fn row_count(&self) -> usize {
        self.tree.summary().rows.max(1)
    }

    /// The view row `row`, clamped to the last; `display` is the snapshot these rows were
    /// made or last edited with.
    pub fn row(&self, display: &DisplaySnapshot, row: usize) -> ViewRow {
        let row = row.min(self.row_count() - 1);
        let mut cursor = self.tree.cursor::<(ViewRows, Start)>();
        cursor.seek(&ViewRows(row), Bias::Right);
        let start = cursor.start().1;
        let end = buffer_end(display);
        match cursor.item() {
            None => ViewRow {
                start: end,
                end,
                line: display.max_point().row,
                column: display.max_point().column,
                continues: false,
            },
            Some(WrapItem::Lines { .. }) => {
                let line = start.lines + (row - start.rows);
                let start = display.display_point_to_offset(DisplayPoint::new(line, 0), Bias::Left);
                ViewRow {
                    start,
                    end: line_end(display, line, end),
                    line,
                    column: 0,
                    continues: false,
                }
            }
            Some(WrapItem::Rows(rows)) => {
                let index = row - start.rows;
                let before: usize = rows[..index].iter().map(|row| row.len as usize).sum();
                let ended = rows[..index].iter().filter(|row| !row.continues).count();
                let offset = start.bytes + before;
                let this = rows[index];
                ViewRow {
                    start: offset,
                    end: offset + this.len as usize,
                    line: start.lines + ended,
                    column: display.offset_to_display_point(offset).column,
                    continues: this.continues,
                }
            }
        }
    }

    /// Where view row `row` starts and ends, and whether its line goes on past it, without
    /// the column a [`ViewRow`] gives, which costs a scan of its line.
    pub(crate) fn row_span(&self, display: &DisplaySnapshot, row: usize) -> (usize, usize, bool) {
        let mut cursor = self.tree.cursor::<(ViewRows, Start)>();
        cursor.seek(&ViewRows(row), Bias::Right);
        let start = cursor.start().1;
        match cursor.item() {
            Some(WrapItem::Rows(rows)) => {
                let index = row - start.rows;
                let offset = start.bytes
                    + rows[..index]
                        .iter()
                        .map(|row| row.len as usize)
                        .sum::<usize>();
                (
                    offset,
                    offset + rows[index].len as usize,
                    rows[index].continues,
                )
            }
            _ => {
                let row = self.row(display, row);
                (row.start, row.end, row.continues)
            }
        }
    }

    /// The view row that holds `offset`; the last one at or past the end.
    pub fn row_of(&self, display: &DisplaySnapshot, offset: usize) -> usize {
        let mut cursor = self.tree.cursor::<(Bytes, Start)>();
        cursor.seek(&Bytes(offset), Bias::Right);
        let start = cursor.start().1;
        match cursor.item() {
            None => self.row_count() - 1,
            Some(WrapItem::Lines { .. }) => {
                let line = display.offset_to_display_point(offset).row;
                start.rows + (line - start.lines)
            }
            Some(WrapItem::Rows(rows)) => {
                let mut at = start.bytes;
                for (index, row) in rows.iter().enumerate() {
                    at += row.len as usize;
                    if offset < at {
                        return start.rows + index;
                    }
                }
                start.rows + rows.len() - 1
            }
        }
    }
}

/// How many view rows `display` takes from its start in a window `columns` wide, as its wrap
/// breaks them, counting no more than `most`: it reads only those rows.
pub(crate) fn rows_at_most(display: &DisplaySnapshot, columns: usize, most: usize) -> usize {
    let fit = Fit::new(columns, false);
    RowStream::new(display, 0, fit).take(most).count().max(1)
}

#[cfg(test)]
mod tests;
