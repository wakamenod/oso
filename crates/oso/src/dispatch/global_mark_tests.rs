//! Scenario tests of `M-@` and the global mark ring, `C-x C-SPC`, with their keys, through the
//! [`Dispatcher`] (ADR 0190, step 8).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::command::Reach;
use oso_core::editor::Editor;
use oso_core::selection::MarkActive;

use super::Given;
use super::motion_tests::{at, cursors, send};
use crate::commands::kept_messages;

/// Each cursor's region, cursor to mark.
fn regions(editor: &Editor) -> Vec<(usize, Option<usize>)> {
    let set = &editor.window().selections;
    set.iter()
        .map(|selection| (selection.cursor, selection.mark))
        .collect()
}

fn shown(editor: &Editor) -> MarkActive {
    editor.window().selections.mark_active()
}

fn ring(editor: &Editor) -> Vec<usize> {
    editor.window().selections.mark_ring().iter().collect()
}

fn last_message(editor: &Editor) -> Option<String> {
    kept_messages(editor).pop()
}

#[test]
fn m_at_marks_the_next_word_at_every_cursor_and_shows_the_region() {
    let (mut d, mut editor) = at("one two three\nfour five six\n", &[0, 14]);
    send(&mut d, &mut editor, "C-SPC C-SPC");
    assert!(send(&mut d, &mut editor, "M-@").redraw);
    assert_eq!(regions(&editor), [(0, Some(3)), (14, Some(18))]);
    assert_eq!(shown(&editor), MarkActive::Active);
    assert_eq!(ring(&editor), [0], "the old mark went to the ring");
    assert_eq!(last_message(&editor).as_deref(), Some("Mark set"));
    assert_eq!(cursors(&editor), [0, 14], "the cursors stay");
}

#[test]
fn m_at_again_or_with_the_region_shown_marks_more_words() {
    let (mut d, mut editor) = at("one two three four\n", &[0]);
    send(&mut d, &mut editor, "M-@ M-@");
    assert_eq!(regions(&editor), [(0, Some(7))], "repeated");
    send(&mut d, &mut editor, "M-2 M-@");
    assert_eq!(regions(&editor), [(0, Some(18))], "N more");
    send(&mut d, &mut editor, "C-g C-e M-b C-SPC C-e");
    assert_eq!(regions(&editor), [(18, Some(14))]);
    send(&mut d, &mut editor, "M-@");
    assert_eq!(
        regions(&editor),
        [(18, Some(8))],
        "the region shows and its mark is before the cursor: it goes on backward"
    );
}

#[test]
fn m_at_with_a_negative_count_marks_the_words_before() {
    let (mut d, mut editor) = at("one two three\n", &[13]);
    send(&mut d, &mut editor, "M-- M-2 M-@");
    assert_eq!(regions(&editor), [(13, Some(4))]);
    assert_eq!(shown(&editor), MarkActive::Active);
}

/// An editor showing `a.txt`, with `b.txt` beside it unshown.
fn two() -> (super::Dispatcher, Editor, BufferId, BufferId) {
    let (d, mut editor) = at("alpha beta gamma\n", &[6]);
    let a = editor.window().buffer();
    let b = editor.add_buffer(Buffer::from_text("b.txt", "one two three\n"));
    (d, editor, a, b)
}

fn place(editor: &Editor) -> (BufferId, usize) {
    let window = editor.window();
    (window.buffer(), window.selections.primary().cursor)
}

#[test]
fn c_x_c_spc_goes_back_to_a_mark_set_in_another_buffer() {
    let (mut d, mut editor, a, b) = two();
    send(&mut d, &mut editor, "C-SPC C-SPC");
    editor.show(b);
    send(&mut d, &mut editor, "M-f C-SPC C-SPC");
    send(&mut d, &mut editor, "C-e");
    assert!(send(&mut d, &mut editor, "C-x C-SPC").redraw);
    assert_eq!(place(&editor), (b, 3), "the newest place");
    send(&mut d, &mut editor, "C-x C-@");
    assert_eq!(place(&editor), (a, 6), "then the one before");
    send(&mut d, &mut editor, "C-x C-SPC");
    assert_eq!(place(&editor), (b, 3), "round again");
}

#[test]
fn c_x_c_spc_with_no_place_says_so() {
    let (mut d, mut editor, _, _) = two();
    send(&mut d, &mut editor, "C-x C-SPC");
    assert_eq!(last_message(&editor).as_deref(), Some("No global mark set"));
}

#[test]
fn the_agent_may_mark_words_and_pop_to_a_place_it_may_be_shown() {
    let (mut d, mut editor, a, b) = two();
    let (_, ran) = d
        .run_for_agent(&mut editor, "mark-word", Given::Nothing, Reach::Look)
        .unwrap();
    assert_eq!(ran, Ok(()));
    assert_eq!(regions(&editor), [(6, Some(10))]);
    editor.show(b);
    let (_, ran) = d
        .run_for_agent(&mut editor, "pop-global-mark", Given::Nothing, Reach::Look)
        .unwrap();
    assert_eq!(ran, Ok(()));
    assert_eq!(place(&editor), (a, 10), "the mark-word's mark");
}

#[test]
fn the_agent_is_refused_a_place_in_a_buffer_with_keys_of_its_own_before_anything_moves() {
    let (mut d, mut editor, a, _) = two();
    let terminal = editor.add_terminal_buffer("*terminal*");
    editor.show(terminal);
    // Its keys go to its program, so its mark is pushed as a command would.
    editor.current_mut().0.selections.push_marks();
    editor.end_command();
    editor.show(a);
    let ring: Vec<_> = editor.global_marks().iter().collect();
    assert_eq!(ring.first().map(|(buffer, _)| *buffer), Some(terminal));
    let before = place(&editor);
    let refused = d.run_for_agent(&mut editor, "pop-global-mark", Given::Nothing, Reach::Look);
    assert!(refused.is_err(), "refused before it runs");
    assert_eq!(place(&editor), before);
    assert_eq!(editor.global_marks().iter().collect::<Vec<_>>(), ring);
}
