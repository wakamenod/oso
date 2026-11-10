//! How rows gather into the bounded items of a [`WrapSnapshot`](super::WrapSnapshot) (ADR
//! 0100): runs of lines that fit, and items of rows that a line that wraps starts and the lines
//! around it share.
//!
//! Must not depend on: the display snapshot or the rules; it sees only rows.

use super::{LINES_PER_ITEM, ROWS_PER_ITEM, Row, WrapItem};

/// Gathers rows and runs of lines into bounded items. A line that wraps opens an item of
/// rows, which takes the lines around it too, so that a few rows of each of many lines share
/// one item; a run of [`LINES_NEAR_ROWS`] lines that fit ends it, and starts an item of lines.
#[derive(Default)]
pub(super) struct Builder {
    items: Vec<WrapItem>,
    /// Lines that fit, not yet in an item.
    run: (u32, usize),
    /// Rows not yet in an item.
    rows: Vec<Row>,
    /// The lengths of lines that fit, read after `rows`, that go with them unless enough
    /// follow to make an item of lines.
    near: Vec<u32>,
    /// Whether the next row starts a logical line.
    line_started: bool,
}

/// How many lines that fit end an item of rows, and start an item of lines.
#[cfg(not(test))]
const LINES_NEAR_ROWS: usize = 16;
/// Small in tests, as [`ROWS_PER_ITEM`] is.
#[cfg(test)]
const LINES_NEAR_ROWS: usize = 2;

impl Builder {
    /// A builder whose first row starts a logical line if `starts_line`.
    pub(super) fn new(starts_line: bool) -> Self {
        Self {
            line_started: starts_line,
            ..Self::default()
        }
    }

    fn flush_run(&mut self) {
        let (lines, bytes) = std::mem::take(&mut self.run);
        if lines > 0 {
            self.items.push(WrapItem::Lines { lines, bytes });
        }
    }

    /// Ends the item of rows, and makes the lines read after it a run of lines.
    fn flush_rows(&mut self) {
        if !self.rows.is_empty() {
            self.flush_run();
            self.items
                .push(WrapItem::Rows(std::mem::take(&mut self.rows).into()));
        }
        for len in std::mem::take(&mut self.near) {
            self.add_run(1, len as usize);
        }
    }

    fn add_run(&mut self, lines: u32, bytes: usize) {
        if self.run.0 + lines > LINES_PER_ITEM {
            self.flush_run();
        }
        self.run.0 += lines;
        self.run.1 += bytes;
    }

    /// Adds `lines` whole lines of `bytes`, each one row.
    pub(super) fn lines(&mut self, lines: u32, bytes: usize) {
        self.flush_rows();
        self.add_run(lines, bytes);
        self.line_started = true;
    }

    /// Pushes `row` into the item of rows, ending it once full.
    fn push(&mut self, row: Row) {
        self.rows.push(row);
        if self.rows.len() == ROWS_PER_ITEM {
            let rows = std::mem::take(&mut self.rows);
            self.flush_run();
            self.items.push(WrapItem::Rows(rows.into()));
        }
    }

    /// Adds one row.
    pub(super) fn row(&mut self, row: Row) {
        if !row.continues && self.line_started {
            // A whole line in one row.
            if self.rows.is_empty() {
                self.lines(1, row.len as usize);
            } else {
                self.near.push(row.len);
                if self.near.len() == LINES_NEAR_ROWS {
                    self.flush_rows();
                }
            }
            return;
        }
        if self.rows.is_empty() {
            self.flush_run();
        }
        for len in std::mem::take(&mut self.near) {
            self.push(Row {
                len,
                continues: false,
                tabs: false,
            });
        }
        self.push(row);
        self.line_started = !row.continues;
    }

    pub(super) fn finish(mut self) -> Vec<WrapItem> {
        self.flush_rows();
        self.flush_run();
        self.items
    }
}
