//! The cells of a window's text area that its cursor boxes, its regions and isearch's
//! matches take (ADR 0071, ADR 0082, ADR 0091), read from the rows a frame shows
//! ([`Screen`], ADR 0104), so that the renderer only fills them.
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.
//!
//! Each screen row is part of a logical line. A range that goes on past a row that continues
//! fills to the row's text width, the cell before the mark's, and takes the mark's column too
//! when a space that hangs there is in it. A range that goes on past a line's last row fills to
//! the area's edge, as Emacs's region face with `:extend`.

use std::ops::Range;

use super::CellRect;
use crate::display_map::{DisplayPoint, DisplaySnapshot};
use crate::motion;
use crate::selection::{Selection, SelectionSet};
use crate::sum_tree::Bias;
use crate::window::{Screen, ScreenRow};

/// The cells of a cursor at `point`, whose cluster takes `cells` cells, in a window with this
/// `text` area showing `screen`. `point`'s column counts a preedit's cells on its line, as the
/// screen's rows do. A cluster that an edge of the area cuts keeps the part in view, and
/// `None` means none of it is.
pub fn cursor_rect(
    text: CellRect,
    screen: &Screen,
    point: DisplayPoint,
    cells: usize,
) -> Option<CellRect> {
    let index = screen.row_of(point).filter(|&index| index < text.rows)?;
    let origin = screen.origin(&screen.rows()[index]);
    let start = point.column.max(origin);
    let end = point
        .column
        .saturating_add(cells.max(1))
        .min(origin.saturating_add(text.columns));
    (start < end).then(|| CellRect {
        row: text.row + index,
        column: text.column + (start - origin),
        rows: 1,
        columns: end - start,
    })
}

/// `point` with a preedit on `screen` counted: what follows the preedit on its line moves
/// right by its cells.
fn pushed(screen: &Screen, point: DisplayPoint, at_preedit: bool) -> DisplayPoint {
    match screen.preedit() {
        Some((at, cells))
            if at.row == point.row
                && (point.column > at.column || (at_preedit && point.column == at.column)) =>
        {
            DisplayPoint::new(point.row, point.column + cells)
        }
        _ => point,
    }
}

/// The cells of the boxes of the cursors of `selections` but the primary, in a window with
/// this `text` area showing `screen`, into `out`, which is emptied first: each over the
/// cluster it is on, as the primary's box, and cut by the area's edges (ADR 0091). A preedit at
/// the primary pushes what follows it on its line. Only the cursors in the columns shown of
/// each row are looked at, found by a binary search, so a frame costs two conversions for each
/// one shown, however many are out of view, also on a long line that does not wrap.
pub fn other_cursor_cells(
    text: CellRect,
    screen: &Screen,
    display: &DisplaySnapshot,
    selections: &SelectionSet,
    out: &mut Vec<CellRect>,
) {
    out.clear();
    if selections.len() < 2 || text.rows == 0 {
        return;
    }
    let all = selections.as_slice();
    for row in screen.rows().iter().take(text.rows) {
        let (low, high) = shown_offsets((text, screen, display), row);
        // The buffer's end is on its last line's last row, where a cursor can be.
        let last = !row.continues && row.line + 1 == display.row_count();
        let high = if last { high + 1 } else { high };
        let first = all.partition_point(|selection| selection.cursor < low);
        for (index, selection) in all.iter().enumerate().skip(first) {
            if selection.cursor >= high {
                break;
            }
            if index == selections.primary_index() {
                continue;
            }
            let point = display.offset_to_display_point(selection.cursor);
            let (point, cells) = motion::cursor_box(display, point, None);
            let point = pushed(screen, point, true);
            out.extend(cursor_rect(text, screen, point, cells));
        }
    }
}

/// The offsets of `row`'s clusters a range or a cursor shows in: the row's own on a row that
/// continues; on a line's last row only those in its columns in view, as a long one that does
/// not wrap may hold many out of view, and its line break, past its last column.
fn shown_offsets(
    (text, screen, display): (CellRect, &Screen, &DisplaySnapshot),
    row: &ScreenRow,
) -> (usize, usize) {
    if row.continues {
        return (row.start, row.end);
    }
    let origin = screen.origin(row);
    let pushed = screen.preedit_on(row).map_or(0, |cells| cells.len());
    let offset = |column| {
        let point = DisplayPoint::new(row.line, column);
        display.display_point_to_offset(point, Bias::Left)
    };
    let low = offset(origin.saturating_sub(pushed)).max(row.start);
    let high = offset(origin.saturating_add(text.columns)).saturating_add(1);
    (low, high.min(row.end))
}

/// The cells of the regions of `selections`, in a window with this `text` area showing
/// `screen`, into `out`: one rectangle for each row of a region in view, and none while the
/// regions are hidden (ADR 0071). A preedit at the primary pushes what follows it on its line,
/// so a region from its point starts after it.
///
/// Each row looks only at the regions in it, found by a binary search on the selections, so a
/// frame costs two conversions for each region a row shows, however long the rows and however
/// many regions are out of view.
pub fn region_cells(
    text: CellRect,
    screen: &Screen,
    display: &DisplaySnapshot,
    selections: &SelectionSet,
    out: &mut Vec<CellRect>,
) {
    out.clear();
    if !selections.mark_active().is_active() {
        return;
    }
    // While the regions show, none overlap, so their ends are in order too.
    let (rows, all) = (0..screen.rows().len(), selections.as_slice());
    range_cells((text, screen, display), rows, all, Selection::range, out);
}

/// The cells of `ranges`, which are in order and do not overlap, in a window as
/// [`region_cells`] takes it, into `out`, after what it holds: as a region's. For isearch's
/// matches (ADR 0082) and trailing whitespace (ADR 0097).
pub fn match_cells(
    text: CellRect,
    screen: &Screen,
    display: &DisplaySnapshot,
    ranges: &[Range<usize>],
    out: &mut Vec<CellRect>,
) {
    let rows = 0..screen.rows().len();
    range_cells((text, screen, display), rows, ranges, Range::clone, out);
}

/// [`match_cells`] on the screen's row `row` alone, for a range known to end there, as a line's
/// trailing whitespace: a row's cost, not a screen's.
pub(crate) fn match_cells_on_row(
    text: CellRect,
    (screen, row): (&Screen, usize),
    display: &DisplaySnapshot,
    range: &Range<usize>,
    out: &mut Vec<CellRect>,
) {
    let one = std::slice::from_ref(range);
    range_cells(
        (text, screen, display),
        row..row + 1,
        one,
        Range::clone,
        out,
    );
}

/// The cells of `items`' ranges, in order and none overlapping, on the screen's rows `rows`,
/// into `out`, as [`region_cells`] says. A range's ends are converted once, however many rows
/// it spans.
fn range_cells<T>(
    (text, screen, display): (CellRect, &Screen, &DisplaySnapshot),
    rows: Range<usize>,
    all: &[T],
    range_of: impl Fn(&T) -> Range<usize>,
    out: &mut Vec<CellRect>,
) {
    let mut first = 0;
    let mut ends: Option<(usize, DisplayPoint, DisplayPoint)> = None;
    let shown = screen.rows().iter().enumerate().take(text.rows);
    for (index, row) in shown.skip(rows.start).take(rows.len()) {
        let origin = screen.origin(row);
        let (low, high) = shown_offsets((text, screen, display), row);
        first += all[first..].partition_point(|item| range_of(item).end <= low);
        for (at, item) in all.iter().enumerate().skip(first) {
            let range = range_of(item);
            if range.start >= high {
                break;
            }
            if range.is_empty() {
                continue;
            }
            let (start, end) = match ends {
                Some((cached, start, end)) if cached == at => (start, end),
                _ => {
                    let start = pushed(screen, display.offset_to_display_point(range.start), true);
                    let end = pushed(screen, display.offset_to_display_point(range.end), false);
                    ends = Some((at, start, end));
                    (start, end)
                }
            };
            let columns = row_columns(screen, row, (start, end));
            let from = columns.start.max(origin);
            let to = columns.end.min(origin.saturating_add(text.columns));
            if from < to {
                out.push(CellRect {
                    row: text.row + index,
                    column: text.column + (from - origin),
                    rows: 1,
                    columns: to - from,
                });
            }
        }
    }
}

/// The columns of `row`'s line that a range from `start` to `end` takes on it: from its start
/// on this row, or the row's; to its end on this row, or past it to the row's text width and
/// the mark's column if a hanging space there is in it, or to the edge past a line's last row.
fn row_columns(
    screen: &Screen,
    row: &ScreenRow,
    (start, end): (DisplayPoint, DisplayPoint),
) -> Range<usize> {
    let from = if start.row == row.line {
        start.column.max(row.column)
    } else {
        row.column
    };
    if end.row == row.line && end.column <= row.end_column {
        return from..end.column;
    }
    // Past a line's last row, or a row with no mark, which draws to the edge.
    if screen.mark(row).is_none() {
        return from..usize::MAX;
    }
    let mark = row.column.saturating_add(screen.width());
    // The next row starts past the mark's column when a space hangs in it.
    let hanging = row.end_column > mark && from <= mark;
    from..mark + usize::from(hanging)
}

#[cfg(test)]
mod tests;
