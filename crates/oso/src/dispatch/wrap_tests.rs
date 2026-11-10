//! Scenario tests of scrolling and motion in a wrapped buffer, with their keys, through the
//! [`Dispatcher`] (ADR 0100, ADR 0103): screens, the wheel, `C-l` and following the cursor
//! count view rows, and motion stays on logical lines.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window. No job runs here: the rows in view are wrapped on this thread.

use oso_core::editor::Editor;

use super::Dispatcher;
use super::motion_tests::{at, cursors, send};

/// One line of 400 words, then a short one: in 20 columns, 19 cells and the mark's, each
/// row holds four words and the space that hangs after them, 20 bytes.
fn paragraph() -> String {
    format!("{}\nnext\n", "word ".repeat(400))
}

/// A dispatcher and an editor 10 rows by 20 columns on [`paragraph`], wrapped, with the
/// cursor at `cursor`.
fn wrapped(cursor: usize) -> (Dispatcher, Editor) {
    let (dispatcher, mut editor) = at(&paragraph(), &[cursor]);
    let id = editor.window().buffer();
    assert!(editor.set_wraps(id, true));
    (dispatcher, editor)
}

#[test]
fn the_view_rows_of_a_wrapped_line_hold_four_words_each() {
    let (_, editor) = wrapped(0);
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let wrap = window.wrap().expect("wrapped");
    for row in 0..10 {
        let row = wrap.row(&display, row);
        assert_eq!((row.start % 20, row.end - row.start), (0, 20), "{row:?}");
        assert!(row.continues);
    }
}

#[test]
fn screens_scroll_by_view_rows() {
    let (mut d, mut editor) = wrapped(0);
    send(&mut d, &mut editor, "C-v");
    assert_eq!(editor.top_row(), 8, "a screen less two rows of context");
    assert_eq!(editor.window().scroll().start, 8 * 20);
    assert_eq!(
        cursors(&editor),
        [8 * 20],
        "the cursor left behind goes to the top"
    );
    send(&mut d, &mut editor, "C-u 3 C-v");
    assert_eq!(editor.top_row(), 11);
    send(&mut d, &mut editor, "M-v");
    assert_eq!(editor.top_row(), 3);
    assert_eq!(cursors(&editor), [11 * 20], "still in view, it stays");
}

#[test]
fn the_view_follows_the_cursor_by_view_rows() {
    let (mut d, mut editor) = wrapped(0);
    send(&mut d, &mut editor, "C-e");
    assert_eq!(
        cursors(&editor),
        [1999],
        "the end of the logical line's text (mwim)"
    );
    assert_eq!(editor.top_row(), 90, "its last view row at the bottom");
    assert_eq!(editor.window().scroll().left, 0, "nothing scrolls sideways");
    send(&mut d, &mut editor, "C-a");
    assert_eq!(editor.top_row(), 0);
}

#[test]
fn line_motion_stays_on_logical_lines() {
    let (mut d, mut editor) = wrapped(45);
    send(&mut d, &mut editor, "C-n");
    assert_eq!(cursors(&editor), [2001 + 4], "the next line, at its end");
    send(&mut d, &mut editor, "C-p C-a");
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn recentering_counts_view_rows() {
    let (mut d, mut editor) = wrapped(50 * 20 + 3);
    send(&mut d, &mut editor, "C-l");
    assert_eq!(editor.top_row(), 45, "the cursor's row in the middle");
    send(&mut d, &mut editor, "C-l");
    assert_eq!(editor.top_row(), 50, "then at the top");
    send(&mut d, &mut editor, "C-l");
    assert_eq!(editor.top_row(), 41, "then at the bottom");
}

#[test]
fn the_wheel_scrolls_by_view_rows_and_stops_at_the_last() {
    let (_, mut editor) = wrapped(0);
    assert!(editor.scroll_view(5));
    assert_eq!(editor.top_row(), 5);
    assert!(editor.scroll_view(1000));
    let (window, buffer) = editor.current();
    let last = window.row_count(&buffer.display()) - 1;
    assert_eq!(
        editor.top_row(),
        last + 1 - 10,
        "the last row at the bottom"
    );
    assert!(!editor.scroll_view_sideways(5));
}

#[test]
fn typing_in_a_wrapped_line_moves_the_rows_after_it() {
    let (mut d, mut editor) = wrapped(0);
    send(&mut d, &mut editor, "a SPC b");
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let wrap = window.wrap().expect("wrapped");
    let first = wrap.row(&display, 0);
    assert_eq!(
        (first.start, first.end),
        (0, 18),
        "\"a bword word word \" breaks before the fourth word"
    );
    assert_eq!(wrap.row(&display, 1).start, 18);
}
