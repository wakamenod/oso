//! The mouse's selection in the grid (ADR 0120, step 4b-2): its text over soft wraps, trailing
//! blanks and wide characters, its marks, and how output moves or clears it.
//!
//! Must not depend on: `app` or a terminal's thread; snapshots are made here.

use oso_platform::process::pty::Size;
use oso_render::{GridRow, GridStyle};
use oso_term::{Attrs, Cell, Row, Snapshot};

use super::Grid;

fn size(rows: u16, columns: u16) -> Size {
    Size {
        rows,
        columns,
        width: 0,
        height: 0,
    }
}

/// A row of `text`, padded with blanks to `columns`; `日` is wide.
fn row(index: u16, text: &str, columns: usize, wrapped: bool) -> Row {
    let mut cells = Vec::new();
    for ch in text.chars() {
        if ch == '日' {
            cells.push(Cell {
                ch,
                attrs: Attrs::WIDE,
                ..Cell::default()
            });
            cells.push(Cell {
                attrs: Attrs::WIDE_SPACER,
                ..Cell::default()
            });
        } else {
            cells.push(Cell {
                ch,
                ..Cell::default()
            });
        }
    }
    cells.resize(columns, Cell::default());
    Row {
        index,
        cells,
        wrapped,
        ..Row::default()
    }
}

/// A screen of `lines`, `history` lines below the scrollback's top, every row sent.
fn screen(lines: &[(&str, bool)], history: usize) -> Snapshot {
    let rows = lines
        .iter()
        .enumerate()
        .map(|(i, (text, wrapped))| row(i as u16, text, 10, *wrapped))
        .collect();
    let mut snapshot = Snapshot {
        size: size(lines.len() as u16, 10),
        rows,
        cursor: oso_term::Cursor::default(),
        modes: oso_term::Modes::default(),
        echo_off: false,
        history,
        scrolled: 0,
        events: Vec::new(),
        dropped_events: 0,
    };
    snapshot.history = history;
    snapshot
}

fn selected(grid: &Grid) -> Vec<String> {
    let mut rows: Vec<GridRow<'_>> = Vec::new();
    grid.rows_into(&mut rows);
    rows.iter()
        .map(|row| {
            (row.cells.iter())
                .map(|c| {
                    if c.style.contains(GridStyle::SELECTED) {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn the_text_joins_soft_wraps_and_drops_trailing_blanks() {
    let mut grid = Grid::default();
    grid.apply(&screen(
        &[("$ echo hel", true), ("lo", false), ("hello  ", false)],
        0,
    ));
    assert!(grid.selection_text().is_none());
    grid.select_from(0, 2);
    assert_eq!(grid.selection_text(), None, "a press alone selects nothing");
    assert!(grid.select_to(2, 9));
    assert_eq!(grid.selection_text().as_deref(), Some("echo hello\nhello"));
    assert_eq!(selected(&grid), ["..########", "##########", "##########"]);
    // Backwards from the cell pressed.
    grid.select_to(0, 0);
    assert_eq!(grid.selection_text().as_deref(), Some("$ e"));
    assert!(grid.clear_selection());
    assert!(!grid.clear_selection(), "nothing left");
    assert_eq!(selected(&grid), ["..........", "..........", ".........."]);
}

#[test]
fn a_wide_character_is_taken_whole() {
    let mut grid = Grid::default();
    grid.apply(&screen(&[("a日本b", false)], 0));
    // Columns: a 0, 日 1–2, 本 3, b 4; from 日's second column takes 日.
    grid.select_from(0, 2);
    grid.select_to(0, 4);
    assert_eq!(grid.selection_text().as_deref(), Some("日本b"));
}

#[test]
fn output_that_scrolls_keeps_the_selection_on_its_text() {
    let mut grid = Grid::default();
    grid.apply(&screen(
        &[("one", false), ("two", false), ("three", false)],
        5,
    ));
    grid.select_from(1, 0);
    grid.select_to(1, 2);
    assert_eq!(grid.selection_text().as_deref(), Some("two"));
    // A line of output scrolls everything up by one.
    grid.apply(&screen(
        &[("two", false), ("three", false), ("four", false)],
        6,
    ));
    assert_eq!(grid.selection_text().as_deref(), Some("two"));
    assert_eq!(selected(&grid)[0], "###.......");
    assert_eq!(selected(&grid)[1], "..........");
}

#[test]
fn output_into_the_selection_or_a_new_size_clears_it() {
    let mut grid = Grid::default();
    grid.apply(&screen(&[("one", false), ("two", false)], 0));
    grid.select_from(0, 0);
    grid.select_to(0, 2);
    // Output elsewhere keeps it.
    grid.apply(&screen(&[("one", false), ("2", false)], 0));
    assert_eq!(grid.selection_text().as_deref(), Some("one"));
    grid.apply(&screen(&[("0ne", false), ("2", false)], 0));
    assert_eq!(grid.selection_text(), None, "written into");
    assert_eq!(selected(&grid)[0], "..........");
    grid.select_from(1, 0);
    grid.select_to(1, 1);
    let mut taller = screen(&[("0ne", false), ("2", false), ("", false)], 0);
    taller.size = size(3, 10);
    grid.apply(&taller);
    assert_eq!(grid.selection_text(), None, "a new size");
    // A new size that sends no row again still takes the marks away.
    grid.select_from(0, 0);
    grid.select_to(0, 2);
    let mut wider = screen(&[], 0);
    wider.size = size(3, 12);
    grid.apply(&wider);
    assert_eq!(selected(&grid)[0], "..........");
}

#[test]
fn a_long_selection_costs_the_rows_shown() {
    let mut grid = Grid::default();
    let lines: Vec<(&str, bool)> = vec![("output", false); 50];
    grid.apply(&screen(&lines, 100_000));
    // Pressed at the screen's top, then output pushes it far into the scrollback.
    grid.select_from(0, 0);
    for history in (100_001..100_200).step_by(10) {
        grid.apply(&screen(&lines, history));
        grid.select_to(49, 5);
        assert!(grid.selection_known() <= 50, "{}", grid.selection_known());
    }
    let text = grid.selection_text().expect("text");
    assert_eq!(text.lines().count(), 50, "only the lines shown");
}

#[test]
fn concealed_cells_are_copied_as_blanks() {
    let mut grid = Grid::default();
    let mut snapshot = screen(&[("ls; rm -rf", false), ("echo", false)], 0);
    for cell in &mut snapshot.rows[0].cells[2..] {
        cell.attrs = Attrs::HIDDEN;
    }
    grid.apply(&snapshot);
    grid.select_from(0, 0);
    grid.select_to(0, 9);
    assert_eq!(grid.selection_text().as_deref(), Some("ls"));
    grid.select_to(1, 3);
    assert_eq!(grid.selection_text().as_deref(), Some("ls\necho"));
    assert_eq!(
        grid.row_text(0).as_deref(),
        Some("ls; rm -rf"),
        "a row's text for the agent channel is a separate decision"
    );
}
