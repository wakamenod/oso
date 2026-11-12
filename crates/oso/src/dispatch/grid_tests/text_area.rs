//! A tiled window's rows in a text-grid snapshot (ADR 0045, ADR 0112): each row, then the
//! lines that mark its cells, the fills behind it, a jump's labels and a preedit's underline.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::ops::Range;

use oso_core::display_map::Preedit;
use oso_core::editor::{Editor, Gutter, JumpCells};
use oso_core::frame::{self, CellRect, WindowRegions};
use oso_core::motion;
use oso_core::window::{Screen, WindowId};

use super::rows::{Style, blank_row, draw_row, screen_row};

/// The gutter's text before each row in view, then `┆`, and the blank that stands for it
/// before a line of marks: nothing with line numbers off (ADR 0097).
fn gutter_rows(
    editor: &Editor,
    (id, columns): (WindowId, usize),
    screen: &Screen,
    rows: usize,
) -> (Vec<String>, String) {
    if columns == 0 {
        return (vec![String::new(); rows], String::new());
    }
    let mut gutter = Gutter::default();
    editor.gutter_into(id, screen, &mut gutter);
    let mut shown: Vec<String> = gutter
        .rows()
        .map(|(text, _)| format!("{text:<columns$}┆"))
        .collect();
    shown.resize(rows, format!("{}┆", " ".repeat(columns)));
    (shown, " ".repeat(columns + 1))
}

/// The rows of the tiled window `id`, as `screen` holds them ([`Editor::screen_of`]), each
/// ended by `┃` while it has a divider at its right. In a window that is not selected, or
/// beneath a prompt, the cursor is hollow and no preedit shows.
pub(super) fn draw_text_area(
    editor: &Editor,
    (id, regions): (WindowId, &WindowRegions),
    screen: &Screen,
    flash: Option<CellRect>,
    lines: &mut Vec<String>,
) {
    let area = regions.text;
    let (gutter, pad) = gutter_rows(editor, (id, regions.gutter.columns), screen, area.rows);
    let divider = if regions.divider.is_some() { "┃" } else { "" };
    let Some(window) = editor.window_by_id(id) else {
        return;
    };
    let buffer = editor.shown(window);
    let mark = if id == editor.window().id() { '^' } else { 'o' };
    let display = buffer.display();
    let point = window.cursor_point(&display);
    let preedit = editor.preedit().filter(|_| screen.preedit().is_some());
    // The cursor box as the renderer draws it.
    let (place, cells) = motion::cursor_box(&display, point, preedit);
    let cursor = frame::cursor_rect(area, screen, place, cells);
    let mut region = Vec::new();
    editor.region_cells(id, screen, &mut region);
    let mut others = Vec::new();
    editor.other_cursor_cells(id, screen, &mut others);
    let mut found = oso_core::editor::MatchCells::default();
    editor.isearch_cells(id, screen, &mut found);
    let (current, matches) = (found.current, found.others);
    let mut jump = JumpCells::default();
    editor.jump_cells(id, screen, &mut jump);
    let (mut trailing, band) = (Vec::new(), editor.current_line_cells(id, screen));
    editor.trailing_cells(id, screen, &mut trailing);
    let whitespace = editor.visual_aids().whitespace;
    for (index, gutter) in gutter.iter().enumerate() {
        let row = screen_row(screen, index, area.columns);
        let cursor = cursor
            .filter(|cells| cells.row == area.row + index)
            .map_or(0..0, |cells| {
                let start = cells.column - area.column;
                start..start + cells.columns
            });
        let spliced = preedit
            .filter(|_| row.as_ref().is_some_and(|row| row.line == point.row))
            .map(|preedit| (point.column, preedit));
        let style = Style {
            cursor: mark,
            whitespace,
        };
        let (text, mut marks) = match &row {
            Some(row) => draw_row(&display, row, cursor, spliced, style),
            None => blank_row(area.columns),
        };
        for cells in others.iter().filter(|cells| cells.row == area.row + index) {
            let start = cells.column - area.column;
            marks[start..start + cells.columns].fill('.');
        }
        lines.push(format!("│{gutter}{text}│{divider}"));
        if marks.iter().any(|&mark| mark != ' ') {
            let marks: String = marks.into_iter().collect();
            lines.push(format!(":{pad}{marks}:"));
        }
        // In the renderer's order, so a region shows over a match, and both over the band.
        let (band, flash) = (band.as_slice(), flash.as_slice());
        let fills = [
            (band, 'l'),
            (&trailing[..], 't'),
            (&current, '#'),
            (&matches, '~'),
            (&jump.matches, '~'),
            (&region, '*'),
            (flash, 'f'),
        ];
        draw_fills(&fills, &jump, (area, index), pad.as_str(), lines);
        let on_row = screen
            .rows()
            .get(index)
            .and_then(|row| Some((row, screen.preedit_on(row)?)));
        if let (Some((at, preedit)), Some((shown, columns))) = (spliced, on_row) {
            let line = underline(at, preedit, (screen.origin(shown), columns), area.columns);
            if line.contains(['-', '=']) {
                lines.push(format!("_{pad}{}", &line['_'.len_utf8()..]));
            }
        }
    }
}

/// The line between `+` that marks the cells of `fills` on the area's row `index`, each over
/// the ones before it, then a jump's labels over them all, as the renderer draws them, and the
/// line between `@` with the labels' keys.
fn draw_fills(
    fills: &[(&[CellRect], char)],
    jump: &JumpCells,
    (area, index): (CellRect, usize),
    pad: &str,
    lines: &mut Vec<String>,
) {
    let mut marks = vec![' '; area.columns];
    for &(cells, mark) in fills {
        for cells in cells.iter().filter(|cells| cells.row == area.row + index) {
            let start = cells.column - area.column;
            marks[start..start + cells.columns].fill(mark);
        }
    }
    let keys = draw_labels(jump, area, index, &mut marks);
    if marks.iter().any(|&mark| mark != ' ') {
        lines.push(format!("+{pad}{}+", marks.into_iter().collect::<String>()));
    }
    if let Some(keys) = keys {
        lines.push(format!("@{pad}{keys}@"));
    }
}

/// The labels of `jump` on the area's row `index`: `J` in `marks` under each label's first
/// key and `j` under the keys after it, as their two faces, and the keys at their cells, if
/// the row has a label.
fn draw_labels(
    jump: &JumpCells,
    area: CellRect,
    index: usize,
    marks: &mut [char],
) -> Option<String> {
    let mut keys = vec![' '; area.columns];
    let on_row = jump
        .labels
        .iter()
        .filter(|label| label.cells.row == area.row + index);
    let mut any = false;
    for label in on_row {
        let start = label.cells.column - area.column;
        let cells = start..start + label.cells.columns;
        for (column, key) in cells.clone().zip(jump.text[label.keys.clone()].chars()) {
            keys[column] = key;
        }
        marks[cells].fill('j');
        marks[start] = 'J';
        any = true;
    }
    any.then(|| keys.into_iter().collect())
}

/// The line under a row with `preedit` at column `at`: `-` under its cells and `=`
/// under the segment being converted, as the renderer's thin and thick underlines, in the
/// row's `shown` columns of its line, from `left` at the area's edge.
fn underline(
    at: usize,
    preedit: &Preedit,
    (left, shown): (usize, Range<usize>),
    columns: usize,
) -> String {
    let mut marks = vec![' '; columns];
    let mut mark = |cells: Range<usize>, mark: char| {
        for column in cells.filter(|column| shown.contains(column)) {
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
