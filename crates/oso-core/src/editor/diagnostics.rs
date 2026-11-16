//! What a language server's diagnostics draw in a window (ADR 0138, step 7): the cells of the
//! wavy lines in each severity's colour, of unnecessary code, faded, and of deprecated code,
//! struck, over the view rows a frame shows (ADR 0104), for the renderer and the text-grid
//! snapshots; and the sign of each line in the gutter.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! - A frame works out each view row's offsets shown, then asks the set once for what meets
//!   them all (`Diagnostics::in_range`): one query, not one a row, as a query costs as many
//!   entries as an early one that covers a long range reaches over. Only a diagnostic that meets
//!   a row's offsets has its ends converted, cut to the rows it meets, so the part of a long
//!   line out of view, and a range around the whole view, cost a search each; a frame costs the
//!   rows and the diagnostics on them, never the buffer's.
//! - The gutter's signs find each diagnostic's line by a search of the rows' offsets, and
//!   convert nothing.
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
use crate::sum_tree::Bias;
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
    /// One row's columns of each list above, in that order ([`ERROR`] to [`DEPRECATED`]), as
    /// they are found.
    row: [Vec<Range<usize>>; 5],
    /// Room for taking one list's columns out of another's.
    scratch: Vec<Range<usize>>,
    /// The diagnostics in view, each one's ends as the screen's points.
    found: Vec<Found>,
    /// Each view row's offsets shown and columns, as [`Rows::span`] gives them.
    spans: Vec<RowSpan>,
}

/// What a view row shows: the offsets of its columns shown, to past its line break on a line's
/// last row, and the columns: shown, after its line's text, and its edge.
#[derive(Clone, Debug)]
struct RowSpan {
    offsets: Range<usize>,
    shown: Range<usize>,
    end: usize,
    edge: usize,
}

/// Where each list is in [`DiagnosticCells::lists`] and its row's columns.
const ERROR: usize = 0;
const WARNING: usize = 1;
const INFORMATION: usize = 2;
const UNNECESSARY: usize = 3;
const DEPRECATED: usize = 4;

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
        cells.spans.clear();
        let mut line_end = None;
        for row in shown {
            let end = match line_end {
                Some((line, end)) if line == row.line => end,
                _ => rows.line_end(row),
            };
            line_end = Some((row.line, end));
            cells.spans.push(rows.span(row, end));
        }
        let (Some(first), Some(last)) = (cells.spans.first(), cells.spans.last()) else {
            return;
        };
        let view = first.offsets.start..last.offsets.end;
        cells.found.clear();
        for diagnostic in diagnostics.in_range(view) {
            let range = diagnostic.range();
            let Some(cut) = cut_to_rows(&cells.spans, range) else {
                continue;
            };
            cells.found.push(Found {
                start: rows.point(cut.start),
                stop: rows.point(cut.end),
                severity: diagnostic.severity(),
                tags: diagnostic.tags(),
            });
        }
        if cells.found.is_empty() {
            return;
        }
        for (index, row) in shown.iter().enumerate() {
            rows.row_cells((index, row), cells);
        }
    }
}

/// Puts the columns the diagnostics of `found` take of `row` in the lists of `lists`, emptied
/// before: of its `shown` columns, with its line's text ending at `end` and its `edge`.
fn collect(
    found: &[Found],
    row: &ScreenRow,
    (shown, end, edge): (&Range<usize>, usize, usize),
    lists: &mut [Vec<Range<usize>>; 5],
) {
    // Lists a diagnostic takes the whole row of: one range each, however many do, so a
    // frame of ranges that span the view costs no sort of them on every row.
    let mut whole = [false; 5];
    for found in found {
        // By start: none after this one starts on this row or before it.
        if found.start.row > row.line {
            break;
        }
        let Some(cut) = columns_of(found, row, (end, edge)) else {
            continue;
        };
        let (columns, empty) = match cut {
            Cut::Columns(columns) => (columns, false),
            Cut::Empty(column) => (column..column + 1, true),
        };
        let columns = columns.start.max(shown.start)..columns.end.min(shown.end);
        if columns.is_empty() {
            continue;
        }
        // An empty range is a place, not text: it gets its line, but nothing faded or struck
        // that would read as a character the buffer does not hold.
        let tags = found.tags;
        let into = [
            match found.severity {
                Severity::Error => Some(ERROR),
                Severity::Warning => Some(WARNING),
                Severity::Information => Some(INFORMATION),
                Severity::Hint => None,
            },
            (tags.unnecessary && !empty).then_some(UNNECESSARY),
            (tags.deprecated && !empty).then_some(DEPRECATED),
        ];
        for list in into.into_iter().flatten() {
            if columns == *shown {
                whole[list] = true;
            } else if !whole[list] {
                lists[list].push(columns.clone());
            }
        }
    }
    for (list, whole) in lists.iter_mut().zip(whole) {
        if whole {
            list.clear();
            list.push(shown.clone());
        }
    }
}

/// `range` cut to the offsets of the rows of `spans` it meets: from the first one's start, if it
/// starts before it, to the last one's end, if it ends after it. `None` if it meets none of
/// them. A range meets a row if it overlaps its offsets, or, empty, is in them. Two searches.
fn cut_to_rows(spans: &[RowSpan], range: Range<usize>) -> Option<Range<usize>> {
    let meets = |span: &&RowSpan| {
        if range.is_empty() {
            span.offsets.contains(&range.start)
        } else {
            span.offsets.start < range.end && range.start < span.offsets.end
        }
    };
    // The rows are in order of their offsets, which do not overlap.
    let first = spans.partition_point(|span| span.offsets.end <= range.start);
    let after = spans.partition_point(|span| span.offsets.start < range.end.max(range.start + 1));
    let rows = spans.get(first..after)?;
    let (first, last) = (rows.iter().find(meets)?, rows.iter().rev().find(meets)?);
    let start = range.start.max(first.offsets.start);
    Some(start..range.end.min(last.offsets.end).max(start))
}

impl Rows<'_> {
    /// What `row` shows, whose line's text ends at column `end`: the offsets of its columns
    /// shown, to past the line break on a line's last row, where an empty range at the end is,
    /// and the columns.
    fn span(&self, row: &ScreenRow, end: usize) -> RowSpan {
        let drawn = self.screen.drawn(row);
        let edge = if row.continues {
            row.end_column
        } else {
            end + 1
        };
        let shown = drawn.start.max(row.column)..drawn.end.min(edge);
        if shown.is_empty() {
            return RowSpan {
                offsets: row.start..row.start,
                shown,
                end,
                edge,
            };
        }
        let offset = |column| {
            let column = self.unpushed(row.line, column);
            let point = DisplayPoint::new(row.line, column);
            self.display.display_point_to_offset(point, Bias::Left)
        };
        let low = offset(shown.start);
        let high = if row.continues || shown.end <= end {
            offset(shown.end)
        } else {
            offset(end).saturating_add(1)
        };
        RowSpan {
            offsets: low..high.max(low),
            shown,
            end,
            edge,
        }
    }

    /// Adds the cells of the diagnostics on the screen's row `index`, `row`, to `cells`.
    fn row_cells(&self, (index, row): (usize, &ScreenRow), cells: &mut DiagnosticCells) {
        let Some(RowSpan {
            shown, end, edge, ..
        }) = cells.spans.get(index).cloned()
        else {
            return;
        };
        if shown.is_empty() {
            return;
        }
        for list in &mut cells.row {
            list.clear();
        }
        collect(&cells.found, row, (&shown, end, edge), &mut cells.row);
        // A preedit's cells are text being composed, which no diagnostic names.
        if let Some(preedit) = self.screen.preedit_on(row) {
            let scratch = &mut cells.scratch;
            for list in &mut cells.row {
                merge(list);
                subtract(list, std::slice::from_ref(&preedit), scratch);
            }
        }
        for list in &mut cells.row {
            merge(list);
        }
        // Each severity's line only where no more severe one's is.
        let (row_lists, scratch) = (&mut cells.row, &mut cells.scratch);
        let [errors, warnings, informations, ..] = row_lists;
        subtract(warnings, errors, scratch);
        subtract(informations, errors, scratch);
        subtract(informations, warnings, scratch);
        let origin = self.screen.origin(row);
        let DiagnosticCells {
            error,
            warning,
            information,
            unnecessary,
            deprecated,
            row: found,
            ..
        } = cells;
        // In the order of the indices, as `lists` gives them.
        let lists = [error, warning, information, unnecessary, deprecated];
        for (columns, out) in found.iter().zip(lists) {
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

    /// The display column of the screen's `column` on `line`, without a preedit's cells.
    fn unpushed(&self, line: usize, column: usize) -> usize {
        match self.screen.preedit() {
            Some((at, cells)) if at.row == line && column > at.column => {
                column.saturating_sub(cells).max(at.column)
            }
            _ => column,
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
/// or the line's text's `end`, within `edge`, the next row's column or past the line break; or
/// the one cell it starts at where it is empty, or holds only the line break. `None` where it
/// takes none of the row.
fn columns_of(found: &Found, row: &ScreenRow, (end, edge): (usize, usize)) -> Option<Cut> {
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
        return Some(Cut::Columns(from..to));
    }
    // Empty here: one cell, only on the row it starts in, not on the rows after it on its line,
    // which a range that ended before them gives nothing.
    let starts_here = start.row == row.line && start.column >= row.column;
    (starts_here && start.column < edge && start.column <= end).then_some(Cut::Empty(start.column))
}

/// What a diagnostic takes of a row: its columns, or the one cell of a range empty there.
enum Cut {
    Columns(Range<usize>),
    Empty(usize),
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
/// `out`'s room: one walk of each, as both are in order.
fn subtract(ranges: &mut Vec<Range<usize>>, minus: &[Range<usize>], out: &mut Vec<Range<usize>>) {
    if minus.is_empty() || ranges.is_empty() {
        return;
    }
    out.clear();
    let mut cuts = minus.iter().peekable();
    for range in ranges.drain(..) {
        let mut start = range.start;
        // Cuts that end before this range cannot meet a later one either.
        while cuts.next_if(|cut| cut.end <= start).is_some() {}
        while let Some(cut) = cuts.peek().filter(|cut| cut.start < range.end) {
            if cut.start > start {
                out.push(start..cut.start);
            }
            start = start.max(cut.end);
            if cut.end > range.end {
                // It reaches into the next range too.
                break;
            }
            cuts.next();
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
