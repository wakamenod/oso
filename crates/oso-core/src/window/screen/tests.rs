//! Tests of a window's screen rows: unwrapped and wrapped rows, the mark, a hanging space,
//! and a preedit laid out in its line as if inserted (ADR 0104).
//!
//! Must not depend on: `editor` or `command`; windows are made here.

use super::*;
use crate::buffer::BufferId;
use crate::display_map::wrap_map::WrapMap;
use crate::display_map::{DisplayMap, TabWidth};
use crate::rope::Rope;
use crate::selection::SelectionSet;
use crate::window::{Scroll, ViewSize};
use proptest::prelude::*;

fn snapshot(text: &str) -> DisplaySnapshot {
    DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
}

/// A window `rows` by `columns` on `text`, wrapped at once if `wraps`, its cursor at `cursor`.
fn make(
    text: &str,
    (rows, columns): (usize, usize),
    wraps: bool,
    cursor: usize,
) -> (Window, DisplaySnapshot) {
    let display = snapshot(text);
    let mut window = Window::new(BufferId(0));
    window.set_size(ViewSize::new(rows, columns));
    if wraps {
        window.set_wrap(Some(WrapMap::new(&display, columns)), &display, 0);
    }
    window.selections = SelectionSet::new(cursor);
    (window, display)
}

fn screen(window: &Window, display: &DisplaySnapshot, preedit: Option<&Preedit>) -> Screen {
    let mut out = Screen::default();
    window.screen_into(display, preedit, &mut out);
    out
}

/// Each row as (line, column, end column, whether it continues).
fn shape(screen: &Screen) -> Vec<(usize, usize, usize, bool)> {
    screen
        .rows()
        .iter()
        .map(|row| (row.line, row.column, row.end_column, row.continues))
        .collect()
}

const END: usize = usize::MAX;

#[test]
fn unwrapped_each_logical_line_is_a_row_from_the_left_column() {
    let (mut window, display) = make("abc\ndefgh\n\nij", (2, 10), false, 0);
    window.set_scroll(Scroll { start: 4, left: 3 });
    let screen = screen(&window, &display, None);
    assert_eq!(shape(&screen), [(1, 0, END, false), (2, 0, END, false)]);
    assert_eq!(screen.top(), 1);
    assert_eq!(screen.left(), 3);
    assert_eq!(screen.drawn(&screen.rows()[0]), 3..13);
    assert_eq!(screen.mark(&screen.rows()[0]), None);
    assert_eq!(screen.span(), 4..11);
}

#[test]
fn wrapped_rows_end_where_the_next_starts_and_draw_the_mark_after_the_width() {
    // 9 columns: 8 of text and the mark's.
    let (window, display) = make("aaaa bbbb ccccc\nend", (10, 9), true, 0);
    let screen = screen(&window, &display, None);
    assert_eq!(
        shape(&screen),
        [
            (0, 0, 5, true),
            (0, 5, 10, true),
            (0, 10, END, false),
            (1, 0, END, false)
        ]
    );
    let first = &screen.rows()[0];
    assert_eq!(screen.drawn(first), 0..5);
    assert_eq!(screen.mark(first), Some(8));
    assert!(first.starts_line() && !screen.rows()[1].starts_line());
    assert_eq!(
        screen.drawn(&screen.rows()[2]),
        10..19,
        "the last row reaches the edge"
    );
}

#[test]
fn a_hanging_space_is_on_its_row_in_the_marks_column() {
    // 8 cells of text, then a space that hangs.
    let (window, display) = make("aaaaaaaa bb", (10, 9), true, 0);
    let screen = screen(&window, &display, None);
    assert_eq!(shape(&screen), [(0, 0, 9, true), (0, 9, END, false)]);
    assert_eq!(screen.row_of(DisplayPoint::new(0, 8)), Some(0), "the space");
    assert_eq!(
        screen.drawn(&screen.rows()[0]),
        0..8,
        "the mark takes its cell"
    );
    assert_eq!(screen.row_of(DisplayPoint::new(0, 9)), Some(1));
    assert_eq!(screen.row_of(DisplayPoint::new(1, 0)), None, "no such line");
}

#[test]
fn the_rows_start_at_the_row_that_holds_the_top_and_fill_the_window() {
    let (mut window, display) = make("aaaa bbbb cccc dddd", (2, 9), true, 0);
    // In the middle of the second row.
    window.set_scroll(Scroll { start: 7, left: 0 });
    let screen = screen(&window, &display, None);
    assert_eq!(screen.top(), 1);
    assert_eq!(shape(&screen), [(0, 5, 10, true), (0, 10, 15, true)]);
}

#[test]
fn a_preedit_at_a_rows_end_wraps_and_moves_the_rows_after_it_down() {
    let text = "aaaa bbb\nnext";
    let (window, display) = make(text, (10, 9), true, 8);
    let preedit = Preedit::new("xyz".into(), None);
    let screen = screen(&window, &display, Some(&preedit));
    assert_eq!(
        shape(&screen),
        [(0, 0, 5, true), (0, 5, END, false), (1, 0, END, false)]
    );
    assert_eq!(screen.preedit(), Some((DisplayPoint::new(0, 8), 3)));
    let (inserted, display) = make("aaaa bbbxyz\nnext", (10, 9), true, 8);
    assert_eq!(
        shape(&screen),
        shape(&self::screen(&inserted, &display, None))
    );
}

#[test]
fn a_preedit_out_of_view_lays_nothing_out() {
    let (mut window, display) = make("aaaa bbbb cccc dddd", (1, 9), true, 0);
    window.set_scroll(Scroll { start: 10, left: 0 });
    let preedit = Preedit::new("x".repeat(30), None);
    let screen = screen(&window, &display, Some(&preedit));
    assert_eq!(shape(&screen), [(0, 10, 15, true)]);
}

#[test]
fn an_unwrapped_preedit_only_says_where_it_is() {
    let (window, display) = make("abc", (2, 9), false, 1);
    let preedit = Preedit::new("xyz".into(), None);
    let screen = screen(&window, &display, Some(&preedit));
    assert_eq!(shape(&screen), [(0, 0, END, false)]);
    assert_eq!(screen.preedit(), Some((DisplayPoint::new(0, 1), 3)));
}

proptest! {
    /// Whatever the text, the window and the preedit on a row in view, the rows are those of
    /// the text with the preedit inserted, from the same top.
    #[test]
    fn prop_a_preedit_shows_as_if_inserted(
        text in "[a あ。\n]{0,40}",
        composed in "[b い」]{1,10}",
        at in 0usize..64,
        rows in 1usize..8,
        columns in 8usize..13,
    ) {
        let cursor = text.floor_char_boundary(at.min(text.len()));
        let (window, display) = make(&text, (rows, columns), true, cursor);
        let wrap = window.wrap().unwrap();
        prop_assume!(wrap.row_of(&display, cursor) < rows);
        let preedit = Preedit::new(composed.clone(), None);
        let shown = screen(&window, &display, Some(&preedit));
        let mut whole = text.clone();
        whole.insert_str(cursor, &composed);
        let (inserted, display) = make(&whole, (rows, columns), true, cursor);
        prop_assert_eq!(shape(&shown), shape(&screen(&inserted, &display, None)));
    }
}
