//! Scenario tests of the keys that add cursors, through the [`Dispatcher`] (ADR 0037,
//! ADR 0091).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::editor::Editor;

use super::motion_tests::{at, cursors, send, text};
use crate::commands::kept_messages;

fn last_message(editor: &Editor) -> Option<String> {
    kept_messages(editor).pop()
}

#[test]
fn c_greater_marks_the_next_ones_and_typing_replaces_them_all() {
    let (mut d, mut editor) = at("foo bar foo foo", &[1]);
    assert!(send(&mut d, &mut editor, "C-> C->").redraw);
    assert_eq!(cursors(&editor), [3, 11, 15]);
    send(&mut d, &mut editor, "x");
    assert_eq!(text(&editor), "x bar x x");
    assert_eq!(cursors(&editor), [1, 7, 9]);
}

#[test]
fn c_g_hides_the_regions_and_then_leaves_one_cursor() {
    let (mut d, mut editor) = at("ab ab ab", &[0]);
    send(&mut d, &mut editor, "C-> C->");
    send(&mut d, &mut editor, "C-g");
    assert_eq!(cursors(&editor), [2, 5, 8]);
    assert!(!editor.window().selections.mark_active().is_active());
    send(&mut d, &mut editor, "C-a C-g");
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn a_negative_count_removes_and_a_skip_moves_on() {
    let (mut d, mut editor) = at("ab ab ab ab", &[0]);
    send(&mut d, &mut editor, "C-> C->");
    send(&mut d, &mut editor, "M-- C->");
    assert_eq!(cursors(&editor), [2, 5]);
    send(&mut d, &mut editor, "C-M->");
    assert_eq!(cursors(&editor), [2, 8]);
    send(&mut d, &mut editor, "C-<");
    assert_eq!(cursors(&editor), [2, 8], "nothing before the first");
    assert_eq!(last_message(&editor).as_deref(), Some("No more matches"));
}

#[test]
fn c_c_c_less_puts_one_on_every_occurrence() {
    let (mut d, mut editor) = at("ab ab abc ab", &[4]);
    send(&mut d, &mut editor, "C-c C-<");
    assert_eq!(cursors(&editor), [2, 5, 12]);
}

#[test]
fn c_x_spc_puts_one_on_each_line() {
    let (mut d, mut editor) = at("one\ntwo\nthree\n", &[1]);
    send(&mut d, &mut editor, "C-SPC C-n C-n C-x SPC");
    assert_eq!(cursors(&editor), [1, 5, 9]);
    send(&mut d, &mut editor, "-");
    assert_eq!(text(&editor), "o-ne\nt-wo\nt-hree\n");
}

#[test]
fn c_greater_in_a_search_marks_the_match_and_the_next() {
    let (mut d, mut editor) = at("ab AB ab", &[0]);
    send(&mut d, &mut editor, "C-s a b C->");
    assert!(editor.isearch().is_none());
    assert_eq!(cursors(&editor), [2, 5]);
}

#[test]
fn c_c_c_less_in_a_search_marks_every_match() {
    let (mut d, mut editor) = at("ab x ab x ab", &[0]);
    send(&mut d, &mut editor, "C-s a b C-c C-<");
    assert!(editor.isearch().is_none());
    assert_eq!(cursors(&editor), [2, 7, 12]);
}

#[test]
fn another_key_under_c_c_ends_the_search_and_runs_as_it_would() {
    let (mut d, mut editor) = at("ab ab", &[0]);
    send(&mut d, &mut editor, "C-s a b C-c C-g");
    assert!(
        editor.isearch().is_some(),
        "C-g under C-c cancels the prefix alone"
    );
    send(&mut d, &mut editor, "C-c x");
    assert!(editor.isearch().is_none());
    assert_eq!(text(&editor), "ab ab", "C-c x is bound to nothing");
}
