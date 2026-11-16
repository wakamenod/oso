//! What a language server's diagnostics draw in a window (ADR 0138, step 7): the cells of the
//! wavy lines in each severity's colour, of unnecessary code, faded, and of deprecated code,
//! struck, over the view rows a frame shows (ADR 0104), for the renderer and the text-grid
//! snapshots; and the sign of each line in the gutter.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! - A frame asks the set once for what meets the offsets in view (`Diagnostics::in_range`),
//!   converts each one's ends once, and splits them by row, so a frame costs the rows and the
//!   diagnostics in view, never the buffer's. One query, not one a row: a query costs as many
//!   entries as an early one that covers a long range reaches over.
//! - Where diagnostics meet, the most severe one's line is drawn, and only it.
//! - A range stops at its line's text: a line never runs to the window's edge, as a region's
//!   does. An empty range, such as a missing `;`, takes the cell it is at, the line break's at a
//!   line's end, and so does a range of the line break alone.
//! - A hint draws no line and no sign (Q16), but is faded or struck as its tags say.

use std::ops::Range;

use super::Editor;
use crate::diagnostic::{Diagnostics, Severity, Tags};
use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::frame::CellRect;
use crate::window::{Screen, ScreenRow, WindowId};

/// The cells of a frame's diagnostics in a window, and room for working them out, kept from
/// frame to frame so that a frame allocates nothing once they have grown.
#[derive(Debug, Default)]
pub struct DiagnosticCells {
    /// Cells with an error's wavy line.
    pub error: Vec<CellRect>,
    /// Cells with a warning's, where no error's is.
    pub warning: Vec<CellRect>,
    /// Cells with information's, where no error's or warning's is.
    pub information: Vec<CellRect>,
    /// Cells of code a server says is unnecessary, whose text is faded.
    pub unnecessary: Vec<CellRect>,
    /// Cells of code a server says is deprecated, which are struck.
    pub deprecated: Vec<CellRect>,
    /// One row's columns of each list above, in that order, as they are found.
    row: [Vec<Range<usize>>; 5],
    /// Room for taking one list's columns out of another's.
    scratch: Vec<Range<usize>>,
    /// The diagnostics in view, each one's ends as the screen's points.
    found: Vec<Found>,
}

/// A diagnostic in view: its ends as the screen's points, a preedit counted, its severity and
/// its tags.
#[derive(Clone, Copy, Debug)]
struct Found {
    start: DisplayPoint,
    stop: DisplayPoint,
    severity: Severity,
    tags: Tags,
}

impl DiagnosticCells {
    fn clear(&mut self) {
        for cells in self.lists() {
            cells.clear();
        }
    }

    fn lists(&mut self) -> [&mut Vec<CellRect>; 5] {
        [
            &mut self.error,
            &mut self.warning,
            &mut self.information,
            &mut self.unnecessary,
            &mut self.deprecated,
        ]
    }
}

/// The rows of a frame a window shows, with what turns their columns into the frame's cells.
struct Rows<'a> {
    screen: &'a Screen,
    display: &'a DisplaySnapshot,
    text: CellRect,
}

impl Editor {
    /// The cells the diagnostics in view take in the text area of the tiled window `id`, as
    /// `screen` shows it ([`Editor::screen_of`]), into `cells`, emptied first. Every window that
    /// shows a buffer a server follows draws them.
    pub fn diagnostic_cells(&self, id: WindowId, screen: &Screen, cells: &mut DiagnosticCells) {
        cells.clear();
        let Some((_, buffer, regions)) = self.window_in_tree(id) else {
            return;
        };
        let Some(diagnostics) = buffer.diagnostics().filter(|set| !set.is_empty()) else {
            return;
        };
        let display = buffer.display();
        let rows = Rows {
            screen,
            display: &display,
            text: regions.text,
        };
        let shown = &screen.rows()[..screen.rows().len().min(regions.text.rows)];
        let (Some(first), Some(last)) = (shown.first(), shown.last()) else {
            return;
        };
        cells.found.clear();
        // From the first row to past the last one's line break, where an empty range can be.
        for diagnostic in diagnostics.in_range(first.start..last.end.saturating_add(1)) {
            let range = diagnostic.range();
            cells.found.push(Found {
                start: rows.point(range.start),
                stop: rows.point(range.end),
                severity: diagnostic.severity(),
                tags: diagnostic.tags(),
            });
        }
        for (index, row) in shown.iter().enumerate() {
            rows.row_cells((index, row), cells);
        }
    }
}

impl Rows<'_> {
    /// Adds the cells of the diagnostics on the screen's row `index`, `row`, to `cells`.
    fn row_cells(&self, (index, row): (usize, &ScreenRow), cells: &mut DiagnosticCells) {
        let drawn = self.screen.drawn(row);
        let end = self.line_end(row);
        // To the line break's cell on a line's last row, where an empty range at the end is.
        let edge = if row.continues {
            row.end_column
        } else {
            end + 1
        };
        let shown = drawn.start.max(row.column)..drawn.end.min(edge);
        if shown.is_empty() {
            return;
        }
        for list in &mut cells.row {
            list.clear();
        }
        for found in &cells.found {
            let Some(columns) = columns_of(found, row, (end, edge)) else {
                continue;
            };
            let columns = columns.start.max(shown.start)..columns.end.min(shown.end);
            if columns.is_empty() {
                continue;
            }
            let tags = found.tags;
            let lists = [
                match found.severity {
                    Severity::Error => Some(0),
                    Severity::Warning => Some(1),
                    Severity::Information => Some(2),
                    Severity::Hint => None,
                },
                tags.unnecessary.then_some(3),
                tags.deprecated.then_some(4),
            ];
            for list in lists.into_iter().flatten() {
                cells.row[list].push(columns.clone());
            }
        }
        for list in &mut cells.row {
            merge(list);
        }
        // Each severity's line only where no more severe one's is.
        let DiagnosticCells {
            error,
            warning,
            information,
            unnecessary,
            deprecated,
            row: columns,
            scratch,
            ..
        } = cells;
        let [errors, warnings, informations, _, _] = columns;
        subtract(warnings, errors, scratch);
        subtract(informations, errors, scratch);
        subtract(informations, warnings, scratch);
        let origin = self.screen.origin(row);
        let lists = [error, warning, information, unnecessary, deprecated];
        for (columns, out) in columns.iter().zip(lists) {
            out.extend(columns.iter().map(|columns| CellRect {
                row: self.text.row + index,
                column: self.text.column + (columns.start - origin),
                rows: 1,
                columns: columns.len(),
            }));
        }
    }

    /// `offset`'s point, with a preedit on its line counted, as the screen's columns are.
    fn point(&self, offset: usize) -> DisplayPoint {
        let point = self.display.offset_to_display_point(offset);
        match self.screen.preedit() {
            Some((at, cells)) if at.row == point.row && point.column > at.column => {
                DisplayPoint::new(point.row, point.column + cells)
            }
            _ => point,
        }
    }

    /// The column after `row`'s line's text, with a preedit on it counted.
    fn line_end(&self, row: &ScreenRow) -> usize {
        let cells = self.display.row_cells(row.line);
        match self.screen.preedit() {
            Some((at, preedit)) if at.row == row.line => cells + preedit,
            _ => cells,
        }
    }
}

/// The columns of `row`'s line that `found` takes on it, from its start or the row's, to its end
/// or the line's text's `end`, within `edge`, the next row's column or past the line break: one
/// cell where it is empty, or holds only the line break. `None` where it takes none of the row.
fn columns_of(found: &Found, row: &ScreenRow, (end, edge): (usize, usize)) -> Option<Range<usize>> {
    let (start, stop) = (found.start, found.stop);
    if start.row > row.line || stop.row < row.line {
        return None;
    }
    let from = if start.row == row.line {
        start.column.max(row.column)
    } else {
        row.column
    };
    let to = if stop.row == row.line {
        stop.column.min(end)
    } else {
        end
    };
    let to = to.min(edge);
    if from < to {
        return Some(from..to);
    }
    // Empty here: one cell where it starts, if it starts on this row.
    (start.row == row.line && from < edge && from <= end).then(|| from..from + 1)
}

/// Sorts `ranges` and joins those that meet.
fn merge(ranges: &mut Vec<Range<usize>>) {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut kept: usize = 0;
    for at in 0..ranges.len() {
        let range = ranges[at].clone();
        match kept.checked_sub(1).map(|last| &mut ranges[last]) {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => {
                ranges[kept] = range;
                kept += 1;
            }
        }
    }
    ranges.truncate(kept);
}

/// Takes `minus`'s columns out of `ranges`, both sorted and merged, keeping them so, with
/// `out`'s room.
fn subtract(ranges: &mut Vec<Range<usize>>, minus: &[Range<usize>], out: &mut Vec<Range<usize>>) {
    if minus.is_empty() || ranges.is_empty() {
        return;
    }
    out.clear();
    for range in ranges.drain(..) {
        let mut start = range.start;
        for cut in minus {
            if cut.end <= start || cut.start >= range.end {
                continue;
            }
            if cut.start > start {
                out.push(start..cut.start);
            }
            start = start.max(cut.end);
        }
        if start < range.end {
            out.push(start..range.end);
        }
    }
    std::mem::swap(ranges, out);
}

/// The lines in view of `screen`'s first `rows` rows on which diagnostics start, each with the
/// diagnostic's severity, hints left out, sorted, into `out`, emptied first: what the gutter's
/// signs are (Q2), from one query of the set for the view.
pub(super) fn starts_in_view(
    diagnostics: &Diagnostics,
    display: &DisplaySnapshot,
    (screen, rows): (&Screen, usize),
    out: &mut Vec<(usize, Severity)>,
) {
    out.clear();
    let shown = &screen.rows()[..screen.rows().len().min(rows)];
    let (Some(first), Some(last)) = (shown.first(), shown.last()) else {
        return;
    };
    let found = diagnostics.in_range(first.start..last.end.saturating_add(1));
    for diagnostic in found.filter(|diagnostic| diagnostic.severity() != Severity::Hint) {
        let line = display
            .offset_to_display_point(diagnostic.range().start)
            .row;
        out.push((line, diagnostic.severity()));
    }
    out.sort_unstable();
}

/// The sign of `line`, the most severe of `starts` on it, as [`starts_in_view`] gives them.
pub(super) fn line_sign(starts: &[(usize, Severity)], line: usize) -> Option<Severity> {
    // Sorted by line, then the most severe first.
    let at = starts.partition_point(|&(other, _)| other < line);
    starts
        .get(at)
        .filter(|&&(other, _)| other == line)
        .map(|&(_, severity)| severity)
}

#[cfg(test)]
mod tests;
