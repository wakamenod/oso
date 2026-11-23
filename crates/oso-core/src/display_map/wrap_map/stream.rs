//! Where each row starts, one after the other from a row's start, as a build, an edit and the
//! rows near a view read them: a line's clusters through the rules (`breaks.rs`), or a line
//! found to fit from its bytes.
//!
//! Must not depend on: the tree of rows, windows or the renderer.

use super::breaks::{Cluster, RowEnd, Rows};
use super::fit::Fit;
use super::indent::LineIndent;
use super::{Row, buffer_end, line_end};
use crate::display_map::{DisplayCluster, DisplayPoint, DisplaySnapshot};

/// The clusters of a row as the rules see them.
type Clustered<'a> =
    std::iter::Map<crate::display_map::tab_map::RowClusters<'a>, fn(DisplayCluster<'a>) -> Cluster>;

/// Where each row the edit or the build reads starts, as [`breaks`] finds them, from a row's
/// start to the end of the buffer.
pub(super) struct RowStream<'a> {
    display: &'a DisplaySnapshot,
    fit: Fit,
    /// The logical line being read, and its rows.
    pub(super) line: usize,
    rows: Option<Rows<'a, Clustered<'a>>>,
    /// A line in one row, found with no cluster read: where it ends and whether it holds a
    /// tab.
    pub(super) whole: Option<(usize, bool)>,
    /// Where the next row starts, and its cell column on its line.
    at: usize,
    column: usize,
    end: usize,
    /// Whether a line may be found to fit from its bytes; off in tests that compare it with
    /// a scan of its clusters.
    pub(super) plain: bool,
}

impl<'a> RowStream<'a> {
    /// The rows of `display` from `start`, a row's start, laid out as `fit` says.
    pub(super) fn new(display: &'a DisplaySnapshot, start: usize, fit: Fit) -> Self {
        let point = display.offset_to_display_point(start);
        let mut stream = Self {
            display,
            fit,
            line: point.row,
            rows: None,
            whole: None,
            at: start,
            column: point.column,
            end: buffer_end(display),
            plain: true,
        };
        stream.open(point);
        stream
    }

    /// Starts reading the line of `point` from its column.
    pub(super) fn open(&mut self, point: DisplayPoint) {
        let line_end = line_end(self.display, point.row, self.end);
        if point.column == 0
            && self.plain
            && let Some(tabs) = self.fits_plainly((self.at, line_end))
        {
            self.whole = Some((line_end, tabs));
            self.rows = None;
            return;
        }
        let clusters: Clustered<'a> = self.display.clusters(point).map(Cluster::of);
        let indent = LineIndent::of(self.fit, self.display, point.row);
        self.rows = Some(Rows::new(clusters, indent, line_end));
    }

    /// Whether the line from `start` to `end`, its line break included, fits in a row with no
    /// rule to read: always unwrapped, and otherwise when it is printable ASCII and tabs, each
    /// character a cluster of its own. Says too whether it holds a tab.
    fn fits_plainly(&self, (start, end): (usize, usize)) -> Option<bool> {
        if !self.fit.wraps() {
            return Some(true);
        }
        let len = end - start;
        if len > self.fit.width + 2 {
            return None;
        }
        let tab_width = self.display.tab_width().get() as usize;
        let (mut column, mut tabs, mut carriage) = (0, false, false);
        let bytes = self.display.text_in(start..end).flat_map(str::bytes);
        for (index, byte) in bytes.enumerate() {
            if carriage && byte != b'\n' {
                // A lone `\r` is an escape of two cells.
                return None;
            }
            match byte {
                b'\t' => {
                    tabs = true;
                    column += tab_width - column % tab_width.max(1);
                }
                0x20..=0x7e => column += 1,
                // The line break that ends it, `\n` or `\r\n`.
                b'\n' if index + 1 == len => {}
                b'\r' if index + 2 == len => carriage = true,
                _ => return None,
            }
        }
        (column <= self.fit.width).then_some(tabs)
    }

    /// Where the next row will start, and its cell column on its line.
    pub(super) fn position(&self) -> (usize, usize) {
        (self.at, self.column)
    }
}

impl Iterator for RowStream<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            if let Some((end, tabs)) = self.whole.take() {
                let row = Row {
                    len: u32::try_from(end - self.at).unwrap_or(u32::MAX),
                    continues: false,
                    tabs,
                };
                self.at = end;
                self.column = 0;
                if self.line + 1 < self.display.row_count() {
                    self.line += 1;
                    self.open(DisplayPoint::new(self.line, 0));
                }
                return Some(row);
            }
            let rows = self.rows.as_mut()?;
            let Some(RowEnd {
                end,
                continues,
                next_column,
                tabs,
            }) = rows.next()
            else {
                if self.line + 1 >= self.display.row_count() {
                    self.rows = None;
                    return None;
                }
                self.line += 1;
                self.open(DisplayPoint::new(self.line, 0));
                continue;
            };
            let row = Row {
                len: u32::try_from(end - self.at).unwrap_or(u32::MAX),
                continues,
                tabs,
            };
            self.at = end;
            self.column = next_column;
            if !continues && self.line + 1 >= self.display.row_count() {
                // The last line's last row: nothing follows.
                self.rows = None;
            }
            return Some(row);
        }
    }
}
