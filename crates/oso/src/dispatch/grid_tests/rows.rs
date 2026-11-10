//! One row of a text-grid snapshot (ADR 0045): its text, cell by cell, and the line of
//! marks below it, for escapes, boxes and their ends, wide clusters, the wrap mark and the
//! cursor. A row is a screen row, part of a logical line, as the renderer draws it (ADR 0104).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::ops::{ControlFlow, Range};

use oso_core::display_map::{
    CellRun, CellStyle, DisplayPoint, DisplaySnapshot, Preedit, row_clusters,
};
use oso_core::window::Screen;

/// The left half of a wide cluster cut off by the right edge of the window.
const LEFT_HALF: char = '▌';
/// The right half of a wide cluster cut off by the left edge of the window.
const RIGHT_HALF: char = '▐';

/// How a row is drawn beyond its text: the mark under the cursor box, and whether
/// whitespace is marked, as the renderer draws a buffer's rows with it (ADR 0097).
#[derive(Clone, Copy)]
pub(super) struct Style {
    pub(super) cursor: char,
    pub(super) whitespace: bool,
}

/// What a row of a snapshot shows of its logical line.
pub(super) struct RowView {
    pub(super) line: usize,
    /// The columns of the line in view, from the area's left edge.
    pub(super) view: Range<usize>,
    /// The column of the line where its clusters stop: the next row's start, or the wrap's
    /// width on a row that continues (ADR 0100).
    pub(super) limit: usize,
    /// The area's column of the wrap mark, on a row that continues.
    pub(super) mark: Option<usize>,
}

/// What the row `index` of `screen` shows in an area `columns` wide, as the renderer draws
/// it; `None` past the rows shown, as past the buffer's end.
pub(super) fn screen_row(screen: &Screen, index: usize, columns: usize) -> Option<RowView> {
    let row = screen.rows().get(index)?;
    let origin = screen.origin(row);
    Some(RowView {
        line: row.line,
        view: origin..origin + columns,
        limit: if row.continues {
            screen.drawn(row).end
        } else {
            row.end_column
        },
        mark: screen.mark(row).filter(|&column| column < columns),
    })
}

/// The text of one row and a mark for each of its cells. `cursor` holds the cells under the
/// cursor box, counted from the row's left edge, which get `style`'s mark. A `preedit` goes in
/// at its column, with no whitespace marked in it. The wrap mark is `↩`, marked `m`.
pub(super) fn draw_row(
    display: &DisplaySnapshot,
    row: &RowView,
    cursor: Range<usize>,
    preedit: Option<(usize, &Preedit)>,
    style: Style,
) -> (String, Vec<char>) {
    let view = row.view.clone();
    let mut text = String::new();
    let mut marks = vec![' '; view.len()];
    let mut drawn = view.start;
    let end = view.end.min(row.limit);
    row_clusters(
        display,
        DisplayPoint::new(row.line, view.start),
        preedit,
        style.whitespace,
        |cluster, _| {
            if cluster.column >= end {
                return ControlFlow::Break(());
            }
            let mut boxed_end = None;
            cluster.for_each_run(|run| {
                let start = run.column.max(view.start);
                let end = (run.column + run.cells).min(view.end);
                if start < end {
                    let cells = &mut marks[start - view.start..end - view.start];
                    draw_run(&run, start, cells, &mut text);
                    drawn = end;
                }
                if run.style == CellStyle::Boxed {
                    boxed_end = Some(run.column + run.cells);
                }
            });
            // A box's last cell, so that two side by side read as two (ADR 0089).
            if let Some(last) = boxed_end.filter(|end| view.contains(&(end - 1))) {
                marks[last - 1 - view.start] = ']';
            }
            ControlFlow::Continue(())
        },
    );
    if let Some(mark) = row.mark {
        text.extend(std::iter::repeat_n(
            ' ',
            (view.start + mark).saturating_sub(drawn),
        ));
        text.push('↩');
        marks[mark] = 'm';
        drawn = view.start + mark + 1;
    }
    text.extend(std::iter::repeat_n(' ', view.end.saturating_sub(drawn)));
    marks[cursor].fill(style.cursor);
    (text, marks)
}

/// A blank row of `columns` cells, past the buffer's end.
pub(super) fn blank_row(columns: usize) -> (String, Vec<char>) {
    (" ".repeat(columns), vec![' '; columns])
}

/// Writes the visible part of `run`, which starts on screen at column `start`
/// and takes the cells of `marks`.
pub(super) fn draw_run(run: &CellRun<'_>, start: usize, marks: &mut [char], text: &mut String) {
    if run.text == " " {
        text.extend(std::iter::repeat_n(' ', marks.len()));
    } else if run.cells == marks.len() {
        text.push_str(run.text);
        match (run.style, &mut *marks) {
            (CellStyle::Text, [first, second]) => (*first, *second) = ('<', '>'),
            (CellStyle::Escape, _) => marks.fill('~'),
            (CellStyle::Boxed, _) => marks.fill('#'),
            (CellStyle::Whitespace, _) => marks.fill('w'),
            (CellStyle::Text, _) => {}
        }
    } else if start == run.column {
        text.push(LEFT_HALF);
    } else {
        text.push(RIGHT_HALF);
    }
}
