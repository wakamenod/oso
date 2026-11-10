//! The rows a frame shows of a window, from its top (ADR 0100, ADR 0104): built once a frame,
//! and read by everything that draws, so that the renderer, the frame's cells, the visual
//! aids, isearch's highlights, the input method's rectangle, the dev socket's rows and the
//! text-grid snapshots agree on every row.
//!
//! Must not depend on: `editor`, `command` or anything that draws.
//!
//! - A row is part of a logical line: the cell columns of that line from where it starts to
//!   where the next row starts. Unwrapped, each logical line is one row, and the window's left
//!   column says which of its columns show.
//! - A wrapped row that continues draws its text in the wrap's width and the mark in the column
//!   after; a row that does not, as a provisional one, draws up to the window's edge.
//! - With an input method's preedit on a row in view, the preedit's logical line is laid out
//!   again with it in, as if inserted (`wrap_map`'s `spliced`), and its rows' columns count the
//!   preedit's cells, as `row_clusters` draws them. The rows after it in view move down.
//!
//! Building it reads the rows in view and no more: a seek to the top and one conversion a
//! row, plus at most a screen of the preedit's line.

use std::fmt;
use std::ops::Range;

use super::{Rows, Scroll, ViewSize, ViewTop, Window};
use crate::display_map::wrap_map::{SplicedRow, WrapSnapshot};
use crate::display_map::{DisplayPoint, DisplaySnapshot, Preedit};
use crate::motion;

/// How many rows the preedit's line is laid out for past a screen: those an edit's rewrap
/// steps back over before the cursor's row, and one more, so that each row shown knows where
/// the next one starts.
const SPLICED_PAST_SCREEN: usize = 5;

/// One row a frame shows of a window.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ScreenRow {
    /// Its logical line, as a [`DisplayPoint`]'s row.
    pub line: usize,
    /// The cell column on its line where it starts, with a preedit's cells counted on the
    /// preedit's line, as `row_clusters` gives the columns there.
    pub column: usize,
    /// The column where the next row of its line starts; `usize::MAX` on its line's last row.
    pub end_column: usize,
    /// Whether its line goes on in the next row, so it draws the mark.
    pub continues: bool,
    /// Where it starts and where the next row starts, in the buffer. A row that starts in a
    /// preedit starts at the cursor.
    pub start: usize,
    pub end: usize,
}

impl ScreenRow {
    /// Whether it is its logical line's first row, where a line number goes (ADR 0097).
    pub fn starts_line(&self) -> bool {
        self.column == 0
    }
}

impl fmt::Debug for ScreenRow {
    /// Leaves out the offsets and columns, as `ViewRow`'s does (ADR 0100).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenRow")
            .field("continues", &self.continues)
            .finish_non_exhaustive()
    }
}

/// The rows a frame shows of a window, from its top. Reused from frame to frame, so that a
/// frame allocates nothing once it has grown.
#[derive(Default)]
pub struct Screen {
    rows: Vec<ScreenRow>,
    /// The view row of the first, as the window's rows count them.
    top: usize,
    /// The first column shown of each row: the window's left column, 0 while wrapped.
    left: usize,
    /// The text area's columns.
    columns: usize,
    /// The cells a wrapped row's text takes, the mark's column after them; `usize::MAX` when
    /// nothing wraps.
    width: usize,
    /// The point of a preedit and its cells, which push what follows it on its line.
    preedit: Option<(DisplayPoint, usize)>,
    /// Room for laying out the preedit's line.
    spliced: Vec<SplicedRow>,
}

impl fmt::Debug for Screen {
    /// Leaves out the rows (see [`ScreenRow`]'s).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Screen")
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

impl Screen {
    /// The rows from the top.
    pub fn rows(&self) -> &[ScreenRow] {
        &self.rows
    }

    /// The view row of the first, as the window's rows count them: the top's.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The first column shown of each row past its start: the window's left column, 0 while
    /// wrapped.
    pub fn left(&self) -> usize {
        self.left
    }

    /// The text area's columns.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// The cells a wrapped row's text takes, the mark's column after them; `usize::MAX` when
    /// nothing wraps.
    pub(crate) fn width(&self) -> usize {
        self.width
    }

    /// The point a preedit is spliced in at and the cells it takes, which push what follows
    /// it on its line.
    pub fn preedit(&self) -> Option<(DisplayPoint, usize)> {
        self.preedit
    }

    /// The column of `row`'s line drawn at the text area's left edge.
    pub fn origin(&self, row: &ScreenRow) -> usize {
        row.column.saturating_add(self.left)
    }

    /// The columns of its line `row` draws the clusters that start in: to the next row's
    /// start, and no further than the wrap's width on a row that continues, whose mark takes
    /// the column after, or than the window's edge on one that does not.
    pub fn drawn(&self, row: &ScreenRow) -> Range<usize> {
        let start = self.origin(row);
        let cells = if row.continues {
            self.width.min(self.columns)
        } else {
            self.columns
        };
        start..row.end_column.min(start.saturating_add(cells))
    }

    /// The column of `row`'s line where its clusters stop: the next row's start, and no further
    /// than the wrap's width on a row that continues, whose mark takes the column after. Past
    /// the window's edge on a line's last row, whose columns the caller cuts.
    pub fn text_end(&self, row: &ScreenRow) -> usize {
        if row.continues {
            self.drawn(row).end
        } else {
            row.end_column
        }
    }

    /// The columns of `row`'s line that the preedit takes on it, if it shows there.
    pub fn preedit_on(&self, row: &ScreenRow) -> Option<Range<usize>> {
        let (at, cells) = self.preedit.filter(|(at, _)| at.row == row.line)?;
        let start = at.column.max(row.column);
        let end = at.column.saturating_add(cells).min(row.end_column);
        (start < end).then_some(start..end)
    }

    /// The text area's column of `row`'s mark, if it draws one.
    pub fn mark(&self, row: &ScreenRow) -> Option<usize> {
        (row.continues && self.width < self.columns).then_some(self.width)
    }

    /// The index of the row that shows `point`, in the columns of [`ScreenRow::column`], if
    /// one does: on its line, from the row's start to the next row's. A search, so it costs
    /// no more with more rows.
    pub fn row_of(&self, point: DisplayPoint) -> Option<usize> {
        let after = self
            .rows
            .partition_point(|row| (row.line, row.column) <= (point.row, point.column));
        let index = after.checked_sub(1)?;
        let row = &self.rows[index];
        (row.line == point.row && point.column < row.end_column).then_some(index)
    }

    /// The buffer's offsets the rows cover, from the first's start to where the last ends.
    pub fn span(&self) -> Range<usize> {
        match (self.rows.first(), self.rows.last()) {
            (Some(first), Some(last)) => first.start..last.end,
            _ => 0..0,
        }
    }

    /// Fills it with the logical lines of `display` from `top`, unwrapped, as a view of `size`
    /// shows them, for a view no window holds, such as a sample of a theme.
    pub fn show_lines(&mut self, display: &DisplaySnapshot, top: ViewTop, size: ViewSize) {
        let start = motion::line_start(display, top.row.min(display.row_count() - 1));
        let view = ScreenSource {
            rows: Rows::new(display, None),
            scroll: Scroll {
                start,
                left: top.left,
            },
            size,
            cursor: start,
        };
        self.fill(view, display, None);
    }

    /// Empties it, for a frame that shows no rows.
    pub(crate) fn clear(&mut self) {
        self.rows.clear();
        self.preedit = None;
    }

    /// Adds `row`, telling the row before it where it ends if it goes on in this one.
    fn push(&mut self, row: ScreenRow) {
        if let Some(last) = self.rows.last_mut()
            && last.continues
            && last.line == row.line
            && last.end_column == usize::MAX
        {
            last.end_column = row.column;
        }
        self.rows.push(row);
    }
}

impl Window {
    /// Fills `out` with the rows a frame shows of this window over `display`, its buffer as
    /// it shows now, with `preedit` spliced in at the primary cursor if there is one.
    pub(crate) fn screen_into(
        &self,
        display: &DisplaySnapshot,
        preedit: Option<&Preedit>,
        out: &mut Screen,
    ) {
        let view = ScreenSource {
            rows: self.rows(display),
            scroll: self.scroll,
            size: self.size,
            cursor: self.selections.primary().cursor,
        };
        out.fill(view, display, preedit);
    }
}

impl Window {
    /// The view row of the caret of `preedit` at the primary cursor, with the preedit's line
    /// laid out with it in, so that the view follows a preedit that wraps past the window's
    /// last row (ADR 0104). `None` while rows do not wrap. Lays out the line only up to the
    /// caret's row, and no more than a screen past the cursor's.
    pub(crate) fn caret_row(&self, display: &DisplaySnapshot, preedit: &Preedit) -> Option<usize> {
        let wrap = self.wrap().filter(|wrap| wrap.wraps())?;
        let cursor = self.selections.primary().cursor.min(display.len());
        let point = display.offset_to_display_point(cursor);
        let (caret, _) = motion::cursor_box(display, point, Some(preedit));
        let mut spliced = Vec::new();
        let limits = (
            self.size.rows.saturating_add(SPLICED_PAST_SCREEN),
            caret.column,
        );
        let first = wrap.spliced_rows(display, (cursor, preedit), limits, &mut spliced);
        let index = spliced
            .partition_point(|row| row.column <= caret.column)
            .saturating_sub(1);
        Some(first + index)
    }

    /// Where a frame shows the primary cursor, with `preedit` laid out at it: the index of its
    /// row among those [`Window::screen_into`] gives, and the column of its line at the
    /// area's left edge there. As those rows have it, without building them: a seek, and the
    /// preedit's line laid out up to the cursor's row. `None` if its row is out of view.
    pub(crate) fn cursor_place(
        &self,
        display: &DisplaySnapshot,
        preedit: Option<&Preedit>,
    ) -> Option<(usize, usize)> {
        let rows = self.rows(display);
        let cursor = self.selections.primary().cursor.min(display.len());
        let top = rows.row_of(self.scroll.start);
        let row = rows.row_of(cursor);
        if !(top..top.saturating_add(self.size.rows)).contains(&row) {
            return None;
        }
        let Some(wrap) = rows.wrap() else {
            return Some((row - top, self.scroll.left));
        };
        let Some(preedit) = preedit.filter(|_| wrap.wraps()) else {
            let column = display.offset_to_display_point(rows.start(row)).column;
            return Some((row - top, column));
        };
        let point = display.offset_to_display_point(cursor);
        let mut spliced = Vec::new();
        let limits = (
            self.size.rows.saturating_add(SPLICED_PAST_SCREEN),
            point.column,
        );
        let first = wrap.spliced_rows(display, (cursor, preedit), limits, &mut spliced);
        let at = spliced
            .partition_point(|row| row.column <= point.column)
            .checked_sub(1)?;
        let index = if top <= first {
            first - top + at
        } else {
            at.checked_sub(spliced_from(&spliced, top, first, rows, display))?
        };
        (index < self.size.rows).then(|| (index, spliced[at].column))
    }
}

/// What a screen shows rows of: view rows from a top, in a view of a size, with a cursor
/// where a preedit goes.
#[derive(Clone, Copy)]
pub(crate) struct ScreenSource<'a> {
    pub(crate) rows: Rows<'a>,
    pub(crate) scroll: Scroll,
    pub(crate) size: ViewSize,
    pub(crate) cursor: usize,
}

impl Screen {
    /// Fills it with the rows `source` shows over `display`, with `preedit` spliced in at its
    /// cursor if there is one.
    pub(crate) fn fill(
        &mut self,
        source: ScreenSource<'_>,
        display: &DisplaySnapshot,
        preedit: Option<&Preedit>,
    ) {
        fill(source, display, preedit, self);
    }
}

/// [`Screen::fill`] into `out`.
fn fill(
    view: ScreenSource<'_>,
    display: &DisplaySnapshot,
    preedit: Option<&Preedit>,
    out: &mut Screen,
) {
    let ScreenSource {
        rows,
        scroll,
        size,
        cursor,
    } = view;
    let cursor = cursor.min(display.len());
    out.rows.clear();
    out.top = rows.row_of(scroll.start);
    out.columns = size.columns;
    out.left = if rows.wrapped() { 0 } else { scroll.left };
    out.preedit = preedit.map(|preedit| {
        let point = display.offset_to_display_point(cursor);
        (point, preedit.cells())
    });
    let wanted = size.rows;
    let Some(wrap) = rows.wrap() else {
        out.width = usize::MAX;
        let end = out.top.saturating_add(wanted).min(display.row_count());
        // Each line's start is the one before's end: one seek a row.
        let mut start = motion::line_start(display, out.top);
        for line in out.top..end {
            let next = next_line_start(display, line);
            out.push(ScreenRow {
                line,
                column: 0,
                end_column: usize::MAX,
                continues: false,
                start,
                end: next,
            });
            start = next;
        }
        return;
    };
    out.width = wrap.width();
    let top = out.top;
    let cursor_row = rows.row_of(cursor);
    let in_view = top <= cursor_row && cursor_row < top.saturating_add(wanted);
    match preedit.filter(|_| wrap.wraps() && in_view) {
        Some(preedit) => {
            let mut spliced = std::mem::take(&mut out.spliced);
            let limits = (wanted.saturating_add(SPLICED_PAST_SCREEN), usize::MAX);
            let first = wrap.spliced_rows(display, (cursor, preedit), limits, &mut spliced);
            push_rows(out, (wrap, display), top..first, wanted);
            let shown = spliced_from(&spliced, top, first, rows, display);
            let line = display.offset_to_display_point(cursor).row;
            push_spliced(out, &spliced[shown..], line, wanted);
            let last = out.rows.last().copied();
            if let Some(last) = last.filter(|last| !last.continues)
                && last.line + 1 < display.row_count()
            {
                let next = wrap.row_of(display, next_line_start(display, last.line));
                push_rows(out, (wrap, display), next..usize::MAX, wanted);
            }
            out.spliced = spliced;
        }
        None => push_rows(out, (wrap, display), top..usize::MAX, wanted),
    }
    // Where the last row's line goes on, if it does; past the cursor, a preedit's cells
    // come before it.
    let pushed = match out.preedit {
        Some((at, cells)) if wrap.wraps() && in_view => Some((at, cells)),
        _ => None,
    };
    if let Some(last) = out.rows.last_mut()
        && last.continues
        && last.end_column == usize::MAX
    {
        let point = display.offset_to_display_point(last.end);
        last.end_column = match pushed {
            Some((at, cells)) if at.row == point.row && last.end > cursor => point.column + cells,
            _ => point.column,
        };
    }
}

/// Where the line after `line` starts, or the buffer's end.
fn next_line_start(display: &DisplaySnapshot, line: usize) -> usize {
    if line + 1 < display.row_count() {
        motion::line_start(display, line + 1)
    } else {
        display.len()
    }
}

/// Adds the wrap's rows `rows` to `out` until it holds `wanted`.
fn push_rows(
    out: &mut Screen,
    (wrap, display): (&WrapSnapshot, &DisplaySnapshot),
    rows: Range<usize>,
    wanted: usize,
) {
    let end = rows.end.min(wrap.row_count());
    for row in rows.start..end {
        if out.rows.len() >= wanted {
            return;
        }
        let (start, end, continues) = wrap.row_span(display, row);
        let point = display.offset_to_display_point(start);
        out.push(ScreenRow {
            line: point.row,
            column: point.column,
            end_column: usize::MAX,
            continues,
            start,
            end,
        });
    }
}

/// The first of the preedit's line's rows, laid out from the window's row `first`, that a
/// window whose top is its row `top` shows: the one that holds the top's start, or the first
/// if the top is above them.
fn spliced_from(
    spliced: &[SplicedRow],
    top: usize,
    first: usize,
    rows: Rows<'_>,
    display: &DisplaySnapshot,
) -> usize {
    if top <= first {
        return 0;
    }
    // The top's start is before the cursor, where columns are as the buffer has them.
    let column = display.offset_to_display_point(rows.start(top)).column;
    spliced
        .partition_point(|row| row.column <= column)
        .saturating_sub(1)
}

/// Adds the rows of line `line` laid out with the preedit in it to `out`, until it holds
/// `wanted`.
fn push_spliced(out: &mut Screen, spliced: &[SplicedRow], line: usize, wanted: usize) {
    for (index, row) in spliced.iter().enumerate() {
        if out.rows.len() >= wanted {
            return;
        }
        let end_column = match spliced.get(index + 1) {
            Some(next) if row.continues => next.column,
            _ => usize::MAX,
        };
        out.push(ScreenRow {
            line,
            column: row.column,
            end_column,
            continues: row.continues,
            start: row.start,
            end: row.end,
        });
    }
}

#[cfg(test)]
mod tests;
