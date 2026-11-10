//! Tests of the cells cursors, regions and matches take, over screens of unwrapped and
//! wrapped rows (ADR 0071, ADR 0091, ADR 0104).
//!
//! Must not depend on: `editor` or `command`; windows and screens are made here.

use super::*;
use crate::buffer::BufferId;
use crate::display_map::wrap_map::WrapMap;
use crate::display_map::{DisplayMap, Preedit, TabWidth};
use crate::rope::Rope;
use crate::selection::MarkActive;
use crate::window::{Scroll, ViewSize, Window};
use proptest::prelude::*;

fn rect(row: usize, column: usize, rows: usize, columns: usize) -> CellRect {
    CellRect {
        row,
        column,
        rows,
        columns,
    }
}

fn snapshot(text: &str) -> DisplaySnapshot {
    DisplayMap::new(&Rope::from(text), TabWidth::new(4)).snapshot()
}

/// How a test's window shows its text.
#[derive(Clone, Copy)]
struct View {
    rows: usize,
    columns: usize,
    /// The logical line at the top, and the left column.
    top: usize,
    left: usize,
    wraps: bool,
}

const VIEW: View = View {
    rows: 10,
    columns: 20,
    top: 0,
    left: 0,
    wraps: false,
};

/// The screen of a window `view` shows of `display`, with `preedit` at `cursor`.
fn screen(display: &DisplaySnapshot, view: View, preedit: Option<(usize, &Preedit)>) -> Screen {
    let mut window = Window::new(BufferId(0));
    window.set_size(ViewSize::new(view.rows, view.columns));
    if view.wraps {
        window.set_wrap(Some(WrapMap::new(display, view.columns)), display, 0);
    }
    let start = motion::line_start(display, view.top);
    window.set_scroll(Scroll {
        start,
        left: view.left,
    });
    window.selections = SelectionSet::new(preedit.map_or(start, |(cursor, _)| cursor));
    let mut out = Screen::default();
    window.screen_into(display, preedit.map(|(_, preedit)| preedit), &mut out);
    out
}

/// A text of many empty lines, whose columns a cursor may be at anywhere.
fn lines() -> DisplaySnapshot {
    snapshot(&"\n".repeat(300))
}

#[test]
fn the_cursor_cell_moves_with_the_view() {
    let display = lines();
    let screen = screen(
        &display,
        View {
            top: 5,
            left: 2,
            ..VIEW
        },
        None,
    );
    let text = rect(0, 0, 10, 20);
    assert_eq!(
        cursor_rect(text, &screen, DisplayPoint::new(7, 3), 1),
        Some(rect(2, 1, 1, 1))
    );
    assert_eq!(
        cursor_rect(text, &screen, DisplayPoint::new(5, 2), 2),
        Some(rect(0, 0, 1, 2)),
        "a wide cluster takes both cells",
    );
}

#[test]
fn a_cursor_out_of_view_has_no_cell() {
    let display = lines();
    let screen = screen(
        &display,
        View {
            top: 5,
            left: 2,
            ..VIEW
        },
        None,
    );
    let text = rect(0, 0, 10, 20);
    for (row, column) in [(4, 3), (15, 3), (7, 1), (7, 22)] {
        let point = DisplayPoint::new(row, column);
        assert_eq!(cursor_rect(text, &screen, point, 1), None, "{point:?}");
    }
    let empty = rect(0, 0, 0, 20);
    let at_top = self::screen(&display, VIEW, None);
    assert_eq!(
        cursor_rect(empty, &at_top, DisplayPoint::new(0, 0), 1),
        None
    );
}

#[test]
fn a_cursor_rectangle_stops_at_the_edges() {
    let display = lines();
    let text = rect(0, 0, 10, 20);
    let at_top = screen(&display, VIEW, None);
    let point = DisplayPoint::new(0, 19);
    assert_eq!(
        cursor_rect(text, &at_top, point, 2),
        Some(rect(0, 19, 1, 1))
    );
    let left = screen(&display, View { left: 5, ..VIEW }, None);
    let at = |column, cells| cursor_rect(text, &left, DisplayPoint::new(0, column), cells);
    assert_eq!(
        at(4, 2),
        Some(rect(0, 0, 1, 1)),
        "the right half of a wide one"
    );
    assert_eq!(at(3, 6), Some(rect(0, 0, 1, 4)), "the end of a long label");
    assert_eq!(at(3, 2), None, "it ends just before the edge");
    let narrow = screen(
        &display,
        View {
            left: 5,
            columns: 4,
            ..VIEW
        },
        None,
    );
    let cells = cursor_rect(rect(0, 0, 10, 4), &narrow, DisplayPoint::new(0, 3), 64);
    assert_eq!(
        cells,
        Some(rect(0, 0, 1, 4)),
        "cut on both sides, it fills the area"
    );
}

#[test]
fn a_cursor_rectangle_keeps_to_an_area_away_from_the_origin() {
    let display = lines();
    let screen = screen(
        &display,
        View {
            top: 5,
            left: 2,
            ..VIEW
        },
        None,
    );
    let cells = cursor_rect(rect(3, 7, 10, 20), &screen, DisplayPoint::new(6, 1), 2);
    assert_eq!(cells, Some(rect(4, 7, 1, 1)));
}

#[test]
fn huge_positions_do_not_overflow() {
    let display = lines();
    let text = rect(0, 0, 10, 20);
    let far = DisplayPoint::new(0, usize::MAX);
    let left = screen(&display, View { left: 5, ..VIEW }, None);
    assert_eq!(cursor_rect(text, &left, far, usize::MAX), None);
    let farthest = screen(
        &display,
        View {
            left: usize::MAX,
            ..VIEW
        },
        None,
    );
    assert_eq!(cursor_rect(text, &farthest, far, 1), None);
}

#[test]
fn a_cursor_on_a_hanging_space_is_in_the_marks_cell() {
    // 11 columns: 10 of text, then a space that hangs into the mark's.
    let display = snapshot("aaaaaaaaaa bb");
    let wrapped = screen(
        &display,
        View {
            columns: 11,
            wraps: true,
            ..VIEW
        },
        None,
    );
    let text = rect(0, 0, 10, 11);
    let space = DisplayPoint::new(0, 10);
    assert_eq!(
        cursor_rect(text, &wrapped, space, 1),
        Some(rect(0, 10, 1, 1))
    );
    let b = DisplayPoint::new(0, 11);
    assert_eq!(cursor_rect(text, &wrapped, b, 1), Some(rect(1, 0, 1, 1)));
}

mod regions {
    use super::*;

    // Rows: "abcdef", "xy", "", "hello", and the empty last one.
    const TEXT: &str = "abcdef\nxy\n\nhello\n";

    fn cells(
        text: &str,
        view: View,
        parts: &[(usize, usize)],
        region: MarkActive,
        preedit: Option<(usize, &Preedit)>,
    ) -> Vec<(usize, usize, usize)> {
        let display = snapshot(text);
        let selections = parts.iter().map(|&(cursor, mark)| Selection {
            mark: Some(mark),
            ..Selection::at(cursor)
        });
        let mut set = SelectionSet::from_parts(selections.collect(), 0).unwrap();
        set.set_mark_active(region);
        let area = rect(0, 0, view.rows, view.columns);
        let shown = screen(&display, view, preedit);
        let mut out = vec![rect(9, 9, 9, 9)];
        region_cells(area, &shown, &display, &set, &mut out);
        out.iter()
            .inspect(|cell| assert_eq!(cell.rows, 1))
            .map(|cell| (cell.row, cell.column, cell.columns))
            .collect()
    }

    const FIVE: View = View {
        rows: 4,
        columns: 5,
        ..VIEW
    };

    #[test]
    fn a_region_fills_its_rows_and_to_the_edge_where_its_line_break_is() {
        // From `c` to `l` of `hello`, the mark first.
        let found = cells(TEXT, FIVE, &[(13, 2)], MarkActive::Active, None);
        assert_eq!(found, [(0, 2, 3), (1, 0, 5), (2, 0, 5), (3, 0, 2)]);
        let backward = cells(TEXT, FIVE, &[(2, 13)], MarkActive::Shifted, None);
        assert_eq!(backward, found, "either way round");
    }

    #[test]
    fn hidden_or_empty_regions_take_no_cells() {
        assert_eq!(
            cells(TEXT, FIVE, &[(13, 2)], MarkActive::Inactive, None),
            []
        );
        assert_eq!(cells(TEXT, FIVE, &[(3, 3)], MarkActive::Active, None), []);
        // Up to the start of `xy`: only the line break of the row before.
        let nine = View { columns: 9, ..FIVE };
        assert_eq!(
            cells(TEXT, nine, &[(0, 7)], MarkActive::Active, None),
            [(0, 0, 9)]
        );
    }

    #[test]
    fn only_the_part_in_view_is_taken_in_the_areas_cells() {
        let left = View { left: 1, ..FIVE };
        let found = cells(TEXT, left, &[(13, 2)], MarkActive::Active, None);
        assert_eq!(found, [(0, 1, 4), (1, 0, 5), (2, 0, 5), (3, 0, 1)]);
        let down = View { top: 2, ..FIVE };
        let found = cells(TEXT, down, &[(13, 2)], MarkActive::Active, None);
        assert_eq!(found, [(0, 0, 5), (1, 0, 2)]);
        let below = View { top: 3, ..FIVE };
        assert_eq!(
            cells(TEXT, below, &[(1, 0)], MarkActive::Active, None),
            [],
            "above it"
        );
    }

    #[test]
    fn each_region_in_view_takes_its_own_cells() {
        let nine = View { columns: 9, ..FIVE };
        let parts = [(1, 0), (5, 3), (9, 8)];
        let found = cells(TEXT, nine, &parts, MarkActive::Active, None);
        assert_eq!(found, [(0, 0, 1), (0, 3, 2), (1, 1, 1)]);
    }

    #[test]
    fn a_preedit_pushes_a_region_after_it_and_not_one_that_ends_there() {
        let nine = View { columns: 9, ..FIVE };
        let preedit = Preedit::new("1234".into(), None);
        let at = Some((2, &preedit));
        // From the preedit's place to `e`.
        assert_eq!(
            cells(TEXT, nine, &[(2, 4)], MarkActive::Active, at),
            [(0, 6, 2)]
        );
        // From `a` to the preedit's place.
        assert_eq!(
            cells(TEXT, nine, &[(2, 0)], MarkActive::Active, at),
            [(0, 0, 2)]
        );
        let other_row = Some((7, &preedit));
        let found = cells(TEXT, nine, &[(2, 4)], MarkActive::Active, other_row);
        assert_eq!(found, [(0, 2, 2)]);
    }

    /// 11 columns, 10 for text and the mark's.
    const WRAPPED: View = View {
        rows: 4,
        columns: 11,
        top: 0,
        left: 0,
        wraps: true,
    };

    #[test]
    fn a_region_past_a_break_fills_to_the_rows_text_width() {
        // Rows `aaaaaa ` and `bbbbbb`: from the second `a` to the second `b`.
        let text = "aaaaaa bbbbbb";
        let found = cells(text, WRAPPED, &[(1, 9)], MarkActive::Active, None);
        assert_eq!(found, [(0, 1, 9), (1, 0, 2)], "not into the mark's column");
        // Ending right at the break, it fills only the row's text.
        let at_break = cells(text, WRAPPED, &[(1, 7)], MarkActive::Active, None);
        assert_eq!(at_break, [(0, 1, 6)]);
    }

    #[test]
    fn a_hanging_space_in_a_region_takes_the_marks_column() {
        // `aaaaaaaaaa` fills the row and its space hangs in the mark's column.
        let text = "aaaaaaaaaa bb";
        let found = cells(text, WRAPPED, &[(9, 12)], MarkActive::Active, None);
        assert_eq!(found, [(0, 9, 2), (1, 0, 1)]);
        let from_b = cells(text, WRAPPED, &[(11, 12)], MarkActive::Active, None);
        assert_eq!(from_b, [(1, 0, 1)], "nothing of the row before");
    }

    #[test]
    fn a_region_past_a_lines_last_row_fills_to_the_edge() {
        let found = cells(
            "aaaaaa bbbbbb\nc",
            WRAPPED,
            &[(8, 15)],
            MarkActive::Active,
            None,
        );
        assert_eq!(found, [(1, 1, 10), (2, 0, 1)]);
    }

    #[test]
    fn a_wrapping_preedit_pushes_a_region_onto_the_next_row() {
        // `aaaa bb|cc` with `xyz` composed at `|`: rows `aaaa ` and `bbxyzcc`.
        let preedit = Preedit::new("xyz".into(), None);
        let at = Some((7, &preedit));
        let found = cells("aaaa bbcc", WRAPPED, &[(7, 9)], MarkActive::Active, at);
        assert_eq!(found, [(1, 5, 2)]);
    }
}

#[test]
fn a_match_over_a_break_takes_cells_on_both_rows() {
    let display = snapshot("aaaaaa bbbbbb");
    let view = View {
        rows: 4,
        columns: 11,
        wraps: true,
        ..VIEW
    };
    let shown = screen(&display, view, None);
    let mut out = Vec::new();
    let one = std::slice::from_ref(&(5..8));
    match_cells(rect(0, 0, 4, 11), &shown, &display, one, &mut out);
    assert_eq!(out, [rect(0, 5, 1, 5), rect(1, 0, 1, 1)]);
}

#[test]
fn other_cursors_get_boxes_in_view_and_the_primary_none() {
    // Rows: `ab`, `日c`, `de`, `fg`; a view of two rows from the second.
    let display = snapshot("ab\n日c\nde\nfg");
    let view = View {
        rows: 2,
        columns: 10,
        top: 1,
        ..VIEW
    };
    let area = rect(0, 0, 2, 10);
    let shown = screen(&display, view, None);
    let parts = [0, 3, 6, 8, 11].into_iter().map(Selection::at).collect();
    let selections = SelectionSet::from_parts(parts, 2).unwrap();
    let mut out = Vec::new();
    other_cursor_cells(area, &shown, &display, &selections, &mut out);
    // `日` takes two cells, the primary at `c` has none, and rows out of view none.
    assert_eq!(out, [rect(0, 0, 1, 2), rect(1, 0, 1, 1)]);

    // A preedit at the primary before a cursor on its row pushes it.
    let preedit = Preedit::new("xyz".into(), None);
    let shown = screen(&display, view, Some((3, &preedit)));
    let parts = [3, 6].into_iter().map(Selection::at).collect();
    let selections = SelectionSet::from_parts(parts, 0).unwrap();
    other_cursor_cells(area, &shown, &display, &selections, &mut out);
    assert_eq!(out, [rect(0, 5, 1, 1)]);
}

#[test]
fn a_cursor_at_the_end_of_the_buffer_gets_a_box_on_the_last_row() {
    let display = snapshot("ab\ncd");
    let shown = screen(
        &display,
        View {
            rows: 5,
            columns: 10,
            ..VIEW
        },
        None,
    );
    let parts = [0, 5].into_iter().map(Selection::at).collect();
    let selections = SelectionSet::from_parts(parts, 0).unwrap();
    let mut out = Vec::new();
    other_cursor_cells(rect(0, 0, 5, 10), &shown, &display, &selections, &mut out);
    assert_eq!(out, [rect(1, 2, 1, 1)]);
}

proptest! {
    /// The cursor's cells are inside the text area, and are exactly the part of its cluster
    /// in view.
    #[test]
    fn prop_a_cursor_rectangle_is_the_part_in_view(
        rows in 1usize..60, columns in 1usize..100,
        top in 0usize..100, left in 0usize..100,
        row in 0usize..200, column in 0usize..200, cells in 1usize..70,
    ) {
        let display = lines();
        let view = View { rows, columns, top, left, wraps: false };
        let shown = screen(&display, view, None);
        let text = rect(0, 0, rows, columns);
        let found = cursor_rect(text, &shown, DisplayPoint::new(row, column), cells);
        let visible = (left..left + columns)
            .filter(|c| (column..column + cells).contains(c))
            .count();
        let in_rows = (top..top + rows).contains(&row) && row < display.row_count();
        prop_assert_eq!(found.is_some(), in_rows && visible > 0);
        if let Some(cell) = found {
            prop_assert_eq!((cell.rows, cell.columns), (1, visible));
            prop_assert_eq!(cell.row, row - top);
            prop_assert!(cell.column + cell.columns <= columns);
        }
    }
}

#[test]
fn on_a_long_line_only_the_cursors_in_view_get_boxes() {
    // One line of 300 cells scrolled 100 columns right, with cursors either side of the view.
    let display = snapshot(&"x".repeat(300));
    let view = View {
        rows: 2,
        columns: 20,
        left: 100,
        ..VIEW
    };
    let shown = screen(&display, view, None);
    let parts = [0, 50, 105, 119, 120, 250, 300]
        .into_iter()
        .map(Selection::at)
        .collect();
    let selections = SelectionSet::from_parts(parts, 0).unwrap();
    let mut out = Vec::new();
    other_cursor_cells(rect(0, 0, 2, 20), &shown, &display, &selections, &mut out);
    assert_eq!(out, [rect(0, 5, 1, 1), rect(0, 19, 1, 1)]);
}
