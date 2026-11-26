//! Tests of a click moving point (ADR 0183): where a cell of a window's text puts it, and what
//! it does to the cursors, the region, the mark ring and the goal.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use crate::buffer::Buffer;
use crate::editor::Editor;
use crate::editor::wrap_tests::settle;
use crate::frame::{FrameSize, Way};
use crate::selection::{Goal, MarkActive, Selection, SelectionSet};
use crate::window::{Screen, WindowId};

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 40,
    });
    editor
}

fn selected(editor: &Editor) -> WindowId {
    editor.window().id()
}

fn cursor(editor: &Editor, id: WindowId) -> usize {
    editor.window_by_id(id).unwrap().selections.primary().cursor
}

/// Clicks the selected window at `row`, `column` of its text and returns point.
fn click(editor: &mut Editor, row: usize, column: usize) -> usize {
    let id = selected(editor);
    assert!(editor.click_point(id, row, column));
    cursor(editor, id)
}

#[test]
fn a_click_puts_point_on_the_cell_under_it() {
    let mut editor = editor("abc\ndefgh\n");
    assert_eq!(click(&mut editor, 1, 2), 6);
    assert_eq!(click(&mut editor, 0, 0), 0);
}

#[test]
fn past_a_lines_end_point_goes_to_its_end_and_below_the_last_row_to_the_buffers() {
    let mut editor = editor("ab\ncdef\nlast");
    assert_eq!(click(&mut editor, 0, 30), 2);
    assert_eq!(click(&mut editor, 1, 30), 7);
    assert_eq!(click(&mut editor, 2, 30), 12);
    assert_eq!(click(&mut editor, 8, 0), 12);
}

#[test]
fn a_character_of_several_cells_takes_point_to_its_start() {
    // `漢` takes columns 1 and 2, the tab columns 0 to 7.
    let mut editor = editor("a漢b\n\tx\n");
    assert_eq!(click(&mut editor, 0, 2), 1);
    assert_eq!(click(&mut editor, 0, 3), 4);
    assert_eq!(click(&mut editor, 1, 5), 6);
    assert_eq!(click(&mut editor, 1, 8), 7);
}

#[test]
fn a_click_leaves_one_cursor_with_the_region_hidden_the_ring_kept_and_no_goal() {
    let mut editor = editor("one two\nthree four\n");
    let mut set = SelectionSet::from_parts(
        vec![
            Selection {
                cursor: 2,
                mark: Some(0),
                goal: Some(Goal::Column(2)),
            },
            Selection::at(10),
        ],
        0,
    )
    .unwrap();
    set.push_primary_mark(5);
    set.set_mark_active(MarkActive::Active);
    editor.current_mut().0.selections = set;
    let ring: Vec<usize> = editor.window().selections.mark_ring().iter().collect();
    let mark = editor.window().selections.primary().mark;
    assert_eq!(click(&mut editor, 1, 3), 11);
    let selections = &editor.window().selections;
    assert_eq!(selections.len(), 1, "extra cursors go");
    assert!(!selections.mark_active().is_active(), "the region hides");
    assert_eq!(selections.primary().goal, None);
    assert_eq!(
        selections.primary().mark,
        mark,
        "the mark stays where it was"
    );
    let after: Vec<usize> = selections.mark_ring().iter().collect();
    assert_eq!(after, ring, "no mark is pushed");
}

#[test]
fn a_window_scrolled_sideways_counts_from_its_left_column() {
    let mut editor = editor(&format!("{}\n", "x".repeat(200)));
    let id = selected(&editor);
    editor.current_mut().0.selections = SelectionSet::new(100);
    editor.follow_cursor();
    let left = editor.window().scroll().left;
    assert!(left > 0, "the window scrolled sideways");
    assert_eq!(click(&mut editor, 0, 3), left + 3);
    assert_eq!(editor.window_by_id(id).unwrap().scroll().left, left);
}

#[test]
fn on_a_wrapped_row_point_stays_on_that_row() {
    let words = "word ".repeat(30);
    let mut editor = editor(&words);
    let buffer = editor.window().buffer();
    assert!(editor.set_wraps(buffer, true));
    // A row not wrapped yet draws to the window's edge, and its cells count from its start.
    assert_eq!(click(&mut editor, 1, 2), 42);
    settle(&mut editor);
    let mut screen = Screen::default();
    editor.screen_of(selected(&editor), &mut screen);
    let rows = screen.rows().to_vec();
    assert!(rows.len() > 2 && rows[0].continues, "the line wraps");
    assert_eq!(click(&mut editor, 1, 0), rows[1].start);
    assert_eq!(click(&mut editor, 1, 2), rows[1].start + 2);
    // Past the row's last cell: its last character, not the next row's start.
    assert_eq!(click(&mut editor, 0, 39), rows[1].start - 1);
}

#[test]
fn a_click_on_a_window_not_selected_moves_its_point_and_keeps_the_selection() {
    let mut editor = editor("abc\ndef\nghi\n");
    let upper = selected(&editor);
    let lower = editor.split_window(Way::Down, false).unwrap();
    assert!(editor.click_point(lower, 2, 1));
    assert_eq!(cursor(&editor, lower), 9);
    assert_eq!(cursor(&editor, upper), 0);
    assert_eq!(selected(&editor), upper);
}

#[test]
fn a_window_that_is_not_tiled_takes_no_click() {
    let mut editor = editor("abc\n");
    let lower = editor.split_window(Way::Down, false).unwrap();
    editor.select_window(lower);
    editor.delete_window().unwrap();
    assert!(!editor.click_point(lower, 0, 1));
    assert_eq!(cursor(&editor, selected(&editor)), 0);
}
