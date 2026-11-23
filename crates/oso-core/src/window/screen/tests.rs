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
use crate::window::{Scroll, ViewSize, WindowId};
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
    let mut window = Window::new(WindowId::new(0), BufferId(0));
    window.set_size(ViewSize::new(rows, columns));
    if wraps {
        window.set_wrap(Some(WrapMap::new(&display, columns)), &display, 0);
    }
    window.selections = SelectionSet::new(cursor);
    (window, display)
}

/// As [`make`], of a buffer whose rows are indented.
fn make_indented(
    text: &str,
    (rows, columns): (usize, usize),
    cursor: usize,
) -> (Window, DisplaySnapshot) {
    let (mut window, display) = make(text, (rows, columns), false, cursor);
    window.set_wrap(Some(WrapMap::indenting(&display, columns)), &display, 0);
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
    // 11 columns: 10 of text and the mark's.
    let (window, display) = make("aaaaaa bbbbbb cccccc\nend", (10, 11), true, 0);
    let screen = screen(&window, &display, None);
    assert_eq!(
        shape(&screen),
        [
            (0, 0, 7, true),
            (0, 7, 14, true),
            (0, 14, END, false),
            (1, 0, END, false)
        ]
    );
    let first = &screen.rows()[0];
    assert_eq!(screen.drawn(first), 0..7);
    assert_eq!(screen.mark(first), Some(10));
    assert!(first.starts_line() && !screen.rows()[1].starts_line());
    assert_eq!(
        screen.drawn(&screen.rows()[2]),
        14..25,
        "the last row reaches the edge"
    );
}

#[test]
fn a_hanging_space_is_on_its_row_in_the_marks_column() {
    // 10 cells of text, then a space that hangs.
    let (window, display) = make("aaaaaaaaaa bb", (10, 11), true, 0);
    let screen = screen(&window, &display, None);
    assert_eq!(shape(&screen), [(0, 0, 11, true), (0, 11, END, false)]);
    assert_eq!(
        screen.row_of(DisplayPoint::new(0, 10)),
        Some(0),
        "the space"
    );
    assert_eq!(
        screen.drawn(&screen.rows()[0]),
        0..10,
        "the mark takes its cell"
    );
    assert_eq!(screen.mark(&screen.rows()[0]), Some(10));
    assert_eq!(screen.row_of(DisplayPoint::new(0, 11)), Some(1));
    assert_eq!(screen.row_of(DisplayPoint::new(1, 0)), None, "no such line");
}

#[test]
fn a_row_wider_than_the_width_draws_to_the_edge_with_no_mark() {
    // Rows wrapped at 16 columns, in a window narrowed to 11 before they are wrapped again.
    let text = "aaaaaaaaaaaaa bbbbbbbbbbbbb";
    let (mut window, display) = make(text, (4, 16), true, 0);
    window.set_size(ViewSize::new(4, 11));
    let wrap = window.wrap_map_mut().unwrap();
    wrap.spend_budget();
    wrap.set_columns(11, &display);
    let screen = screen(&window, &display, None);
    let first = &screen.rows()[0];
    assert_eq!((first.column, first.end_column), (0, 14), "the old row");
    assert_eq!(screen.mark(first), None);
    assert_eq!(screen.drawn(first), 0..11, "to the edge");
    assert_eq!(screen.text_end(first), 14);
}

#[test]
fn the_rows_start_at_the_row_that_holds_the_top_and_fill_the_window() {
    let (mut window, display) = make("aaaaaa bbbbbb cccccc dddddd", (2, 11), true, 0);
    // In the middle of the second row.
    window.set_scroll(Scroll { start: 9, left: 0 });
    let screen = screen(&window, &display, None);
    assert_eq!(screen.top(), 1);
    assert_eq!(shape(&screen), [(0, 7, 14, true), (0, 14, 21, true)]);
}

#[test]
fn a_preedit_at_a_rows_end_wraps_and_moves_the_rows_after_it_down() {
    let text = "aaaa bbbbb\nnext";
    let (window, display) = make(text, (10, 11), true, 10);
    let preedit = Preedit::new("xyz".into(), None);
    let screen = screen(&window, &display, Some(&preedit));
    assert_eq!(
        shape(&screen),
        [(0, 0, 5, true), (0, 5, END, false), (1, 0, END, false)]
    );
    assert_eq!(screen.preedit(), Some((DisplayPoint::new(0, 10), 3)));
    let (inserted, display) = make("aaaa bbbbbxyz\nnext", (10, 11), true, 10);
    assert_eq!(
        shape(&screen),
        shape(&self::screen(&inserted, &display, None))
    );
}

#[test]
fn a_preedit_out_of_view_lays_nothing_out() {
    let (mut window, display) = make("aaaaaa bbbbbb cccccc dddddd", (1, 11), true, 0);
    window.set_scroll(Scroll { start: 14, left: 0 });
    let preedit = Preedit::new("x".repeat(30), None);
    let screen = screen(&window, &display, Some(&preedit));
    assert_eq!(shape(&screen), [(0, 14, 21, true)]);
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
        columns in 10usize..15,
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

proptest! {
    /// Whatever the text, the top, the window and a preedit, the cursor's place is where its
    /// screen shows it.
    #[test]
    fn prop_the_cursors_place_is_its_screen_rows(
        text in "[a あ。\n-]{0,60}",
        composed in proptest::option::of("[b い」]{1,10}"),
        (at, top) in (0usize..64, 0usize..64),
        rows in 1usize..6,
        columns in 10usize..15,
        (wraps, indents) in proptest::bool::ANY.prop_flat_map(|w| (Just(w), proptest::bool::ANY)),
    ) {
        let cursor = text.floor_char_boundary(at.min(text.len()));
        let (mut window, display) = match indents {
            // Wide enough to wrap; a row further left than it stands is drawn at its column.
            true => make_indented(&text, (rows, columns + 6), cursor),
            false => make(&text, (rows, columns), wraps, cursor),
        };
        let start = text.floor_char_boundary(top.min(text.len()));
        window.set_scroll(Scroll { start, left: 0 });
        let preedit = composed.map(|composed| Preedit::new(composed, None));
        let shown = screen(&window, &display, preedit.as_ref());
        let point = display.offset_to_display_point(cursor);
        let expected = shown
            .row_of(point)
            .map(|row| (row, shown.origin(&shown.rows()[row])));
        prop_assert_eq!(window.cursor_place(&display, preedit.as_ref()), expected);
    }
}

#[test]
fn debug_prints_no_offset_or_column() {
    // Where rows break says how long words are (ADR 0100).
    let (window, display) = make("aaaaaa bbbbbb cccccc", (4, 11), true, 0);
    let shown = screen(&window, &display, None);
    assert_eq!(format!("{shown:?}"), "Screen { rows: 3, .. }");
    let row = format!("{:?}", shown.rows()[1]);
    assert_eq!(row, "ScreenRow { continues: true, .. }");
}

#[test]
fn only_a_space_may_take_the_marks_column() {
    // Rows of 11 cells wrapped at 12 columns, in a window narrowed to 11 before they are
    // wrapped again: their last cell is a letter, which the mark's column does not take.
    let text = "0123456789XYZ0123456789XYZ";
    let (mut window, display) = make(text, (4, 12), true, 0);
    window.set_size(ViewSize::new(4, 11));
    let wrap = window.wrap_map_mut().unwrap();
    wrap.spend_budget();
    wrap.set_columns(11, &display);
    let screen = screen(&window, &display, None);
    let first = &screen.rows()[0];
    assert_eq!((first.column, first.end_column), (0, 11), "the old row");
    assert_eq!(screen.mark(first), None);
    assert_eq!(screen.drawn(first), 0..11, "to the edge");
}

#[test]
fn a_preedit_that_makes_a_break_lets_an_earlier_short_row_take_the_rows_after() {
    // "aa" broke early, as nothing after it could until the line's end; the preedit's "b" may
    // start a row after the spaces, the last of which hangs, so "aa" takes the row after it.
    let text = "。aa   あああ。。 \n。\naaあ。。  。";
    let cursor = text.len() - "。".len();
    let (window, display) = make(text, (6, 10), true, cursor);
    let preedit = Preedit::new("b".into(), None);
    let shown = screen(&window, &display, Some(&preedit));
    assert_eq!(shape(&shown)[3..], [(2, 0, 10, true), (2, 10, END, false)]);
    let mut whole = text.to_string();
    whole.insert(cursor, 'b');
    let (inserted, display) = make(&whole, (6, 10), true, cursor);
    assert_eq!(shape(&shown), shape(&screen(&inserted, &display, None)));
}

#[test]
fn an_indented_row_draws_past_its_indent_with_its_mark_in_the_last_column() {
    // 17 columns: 16 of text and the mark's; the rows it goes on in, 12 at column 4.
    let text = "  - aaaaaaaaaaaa bbbbbbbbbbbb cccccc ddd";
    let (mut window, display) = make_indented(text, (4, 17), 0);
    let shown = screen(&window, &display, None);
    let columns: Vec<_> = shown.rows().iter().map(|row| row.column).collect();
    assert_eq!(columns, [0, 17, 30]);
    let (first, middle, last) = (&shown.rows()[0], &shown.rows()[1], &shown.rows()[2]);
    assert_eq!((first.indent, middle.indent, last.indent), (0, 4, 4));
    // The space after each word hangs in the mark's column.
    assert_eq!(
        (shown.mark(first), shown.mark(middle)),
        (Some(16), Some(16))
    );
    assert_eq!(shown.mark(last), None);
    assert_eq!(shown.origin(middle), 13);
    assert_eq!(shown.drawn(middle), 17..29, "the space hangs");
    assert_eq!(shown.drawn(last), 30..43);
    // The cursor on that row, and on its hanging space.
    for (offset, cell) in [(17, 4), (28, 15), (29, 16)] {
        window.selections = SelectionSet::new(offset);
        let (row, origin) = window.cursor_place(&display, None).unwrap();
        assert_eq!((row, offset - origin), (1, cell));
    }
}

#[test]
fn rows_not_yet_wrapped_again_show_no_text_before_where_their_line_starts() {
    let text = format!("{}\n", "    ▸ Read a word of text and more ".repeat(40));
    let display = snapshot(&text);
    let mut window = Window::new(WindowId::new(0), BufferId(0));
    let check = |window: &Window, columns: usize| {
        let shown = screen(window, &display, None);
        for row in shown.rows().iter().filter(|row| row.column > 0) {
            let cell = shown.drawn(row).start - shown.origin(row);
            assert!(cell >= 6.min(row.column), "at {columns}");
        }
    };
    // Provisional, then wrapped from an anchor, then narrower before its report.
    for columns in [60, 30, 15] {
        window.set_size(ViewSize::new(6, columns));
        let mut wrap = WrapMap::provisional(&display, columns, true);
        window.set_wrap(Some(wrap.clone()), &display, 0);
        check(&window, columns);
        wrap.refill();
        wrap.realize(&display, 700, 4);
        window.set_wrap(Some(wrap.clone()), &display, 0);
        window.set_scroll(Scroll {
            start: 700,
            left: 0,
        });
        check(&window, columns);
        let mut wrap = WrapMap::indenting(&display, 80);
        wrap.set_columns(columns, &display);
        window.set_wrap(Some(wrap), &display, 0);
        check(&window, columns);
    }
}
