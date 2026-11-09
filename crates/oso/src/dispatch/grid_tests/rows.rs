//! One row of a text-grid snapshot (ADR 0045): its text, cell by cell, and the line of
//! marks below it, for escapes, boxes and their ends, wide clusters and the cursor.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::ops::{ControlFlow, Range};

use oso_core::display_map::{
    CellRun, CellStyle, DisplayPoint, DisplaySnapshot, Preedit, row_clusters,
};
use oso_core::window::Scroll;

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

/// The text of one row of `columns` cells and a mark for each of them. `cursor`
/// holds the cells under the cursor box, counted from the row's left edge, which get
/// `style`'s mark. A `preedit` goes in at its column, with no whitespace marked in it.
pub(super) fn draw_row(
    display: &DisplaySnapshot,
    row: usize,
    scroll: Scroll,
    columns: usize,
    cursor: Range<usize>,
    preedit: Option<(usize, &Preedit)>,
    style: Style,
) -> (String, Vec<char>) {
    let view = scroll.left..scroll.left + columns;
    let mut text = String::new();
    let mut marks = vec![' '; columns];
    let mut drawn = view.start;
    row_clusters(
        display,
        DisplayPoint::new(row, view.start),
        preedit,
        |cluster, own| {
            if cluster.column >= view.end {
                return ControlFlow::Break(());
            }
            let mut boxed_end = None;
            cluster.for_each_run_with(style.whitespace && !own, |run| {
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
    text.extend(std::iter::repeat_n(' ', view.end - drawn));
    marks[cursor].fill(style.cursor);
    (text, marks)
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
