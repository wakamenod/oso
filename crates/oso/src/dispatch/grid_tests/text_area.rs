//! The tiled window's rows in a text-grid snapshot (ADR 0045): each row, then the lines
//! that mark its cells, the fills behind it and a preedit's underline.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::ops::Range;

use oso_core::display_map::Preedit;
use oso_core::editor::{Editor, Gutter};
use oso_core::frame::{self, CellRect};
use oso_core::motion;

use super::rows::{Style, draw_row};

/// The gutter's text before each row in view, then `┆`, and the blank that stands for it
/// before a line of marks: nothing with line numbers off (ADR 0097).
fn gutter_rows(editor: &Editor, rows: usize) -> (Vec<String>, String) {
    let columns = editor.frame().window.gutter.columns;
    if columns == 0 {
        return (vec![String::new(); rows], String::new());
    }
    let mut gutter = Gutter::default();
    editor.gutter_into(&mut gutter);
    let mut shown: Vec<String> = gutter
        .rows()
        .map(|(text, _)| format!("{text:<columns$}┆"))
        .collect();
    shown.resize(rows, format!("{}┆", " ".repeat(columns)));
    (shown, " ".repeat(columns + 1))
}

/// The tiled window's rows. Beneath a prompt, its cursor is hollow and it shows no preedit.
pub(super) fn draw_text_area(
    editor: &Editor,
    area: CellRect,
    flash: Option<CellRect>,
    lines: &mut Vec<String>,
) {
    let (gutter, pad) = gutter_rows(editor, area.rows);
    let (window, buffer) = editor.tiled();
    let beneath = editor.prompt().is_some();
    let mark = if beneath { 'o' } else { '^' };
    let display = buffer.display();
    let scroll = window.drawn_top(&display);
    let point = window.cursor_point(&display);
    let preedit = editor.preedit().filter(|_| !beneath);
    // The cursor box as the renderer draws it.
    let (place, cells) = motion::cursor_box(&display, point, preedit);
    let cursor = frame::cursor_rect(area, scroll, place, cells);
    let mut region = Vec::new();
    editor.region_cells(&mut region);
    let mut others = Vec::new();
    editor.other_cursor_cells(&mut others);
    let mut found = oso_core::editor::MatchCells::default();
    editor.isearch_cells(&mut found);
    let (current, matches) = (found.current, found.others);
    let (mut trailing, band) = (Vec::new(), editor.current_line_cells());
    editor.trailing_cells(&mut trailing);
    let whitespace = editor.visual_aids().whitespace;
    for (index, gutter) in gutter.iter().enumerate() {
        let row = scroll.row + index;
        let cursor = cursor
            .filter(|cells| cells.row == area.row + index)
            .map_or(0..0, |cells| {
                let start = cells.column - area.column;
                start..start + cells.columns
            });
        let spliced = preedit
            .filter(|_| row == point.row)
            .map(|preedit| (point.column, preedit));
        let style = Style {
            cursor: mark,
            whitespace,
        };
        let (text, mut marks) =
            draw_row(&display, row, scroll, area.columns, cursor, spliced, style);
        for cells in others.iter().filter(|cells| cells.row == area.row + index) {
            let start = cells.column - area.column;
            marks[start..start + cells.columns].fill('.');
        }
        lines.push(format!("│{gutter}{text}│"));
        if marks.iter().any(|&mark| mark != ' ') {
            let marks: String = marks.into_iter().collect();
            lines.push(format!(":{pad}{marks}:"));
        }
        let mut marks = vec![' '; area.columns];
        // In the renderer's order, so a region shows over a match, and both over the band.
        let (band, flash) = (band.as_slice(), flash.as_slice());
        let fills = [
            (band, 'l'),
            (&trailing[..], 't'),
            (&current, '#'),
            (&matches, '~'),
            (&region, '*'),
        ];
        for (cells, mark) in fills.into_iter().chain([(flash, 'f')]) {
            for cells in cells.iter().filter(|cells| cells.row == area.row + index) {
                let start = cells.column - area.column;
                marks[start..start + cells.columns].fill(mark);
            }
        }
        if marks.iter().any(|&mark| mark != ' ') {
            lines.push(format!("+{pad}{}+", marks.into_iter().collect::<String>()));
        }
        if let Some((at, preedit)) = spliced {
            let line = underline(at, preedit, scroll.left, area.columns);
            lines.push(format!("_{pad}{}", &line['_'.len_utf8()..]));
        }
    }
}

/// The line under a row with `preedit` at column `at`: `-` under its cells and `=`
/// under the segment being converted, as the renderer's thin and thick underlines.
fn underline(at: usize, preedit: &Preedit, left: usize, columns: usize) -> String {
    let mut marks = vec![' '; columns];
    let mut mark = |cells: Range<usize>, mark: char| {
        for column in cells {
            if let Some(cell) = column.checked_sub(left).and_then(|i| marks.get_mut(i)) {
                *cell = mark;
            }
        }
    };
    let (all, segment) = preedit.underlines(at);
    mark(all, '-');
    if let Some(segment) = segment {
        mark(segment, '=');
    }
    format!("_{}_", marks.into_iter().collect::<String>())
}
