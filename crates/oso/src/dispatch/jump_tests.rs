//! Scenario tests of jumps through the [`Dispatcher`] (ADR 0037, ADR 0116): their keys, their
//! keymap in place of the others, a key it does not bind ending the jump without running, a
//! digit at `avy-goto-line`'s labels, `avy-isearch`, and `run-command`.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::editor::{Editor, Status};
use oso_core::jump::{Keys, Stage};

use super::motion_tests::{at, cursors, send, text};

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

fn stage(editor: &Editor) -> Option<Stage> {
    editor.jump().map(|jump| jump.stage())
}

#[test]
fn m_j_reads_a_string_ret_shows_labels_and_a_label_jumps() {
    let (mut d, mut editor) = at("xab ab\nab\n", &[0]);
    assert!(send(&mut d, &mut editor, "M-j a b").redraw);
    assert_eq!(stage(&editor), Some(Stage::Reading));
    assert_eq!(echo(&editor), "Jump: ab");
    send(&mut d, &mut editor, "DEL b RET");
    assert_eq!(stage(&editor), Some(Stage::Labels));
    // Nearest first: 1 on the cursor's row, then 4, then 7 a row down.
    send(&mut d, &mut editor, "s");
    assert!(editor.jump().is_none());
    assert_eq!(cursors(&editor), [4]);
    assert_eq!(text(&editor), "xab ab\nab\n", "no key typed in the buffer");
}

#[test]
fn a_key_no_label_takes_or_the_keymap_does_not_bind_ends_the_jump_and_does_not_run() {
    let (mut d, mut editor) = at("ab ab\n", &[0]);
    send(&mut d, &mut editor, "M-j a RET z");
    assert!(editor.jump().is_none());
    assert_eq!(echo(&editor), "No such candidate");
    assert_eq!(
        (cursors(&editor), text(&editor)),
        (vec![0], "ab ab\n".into())
    );
    send(&mut d, &mut editor, "M-j a RET C-f");
    assert!(editor.jump().is_none());
    assert_eq!(cursors(&editor), [0], "`C-f` did not run");
    // While the string is read, too.
    send(&mut d, &mut editor, "M-j a C-f");
    assert!(editor.jump().is_none());
    assert_eq!(cursors(&editor), [0]);
    assert!(d.pending().is_empty());
}

#[test]
fn c_g_ends_a_jump_with_nothing_moved() {
    let (mut d, mut editor) = at("ab ab\n", &[0]);
    send(&mut d, &mut editor, "M-j a RET C-g");
    assert!(editor.jump().is_none());
    assert_eq!(cursors(&editor), [0]);
    assert_eq!(echo(&editor), "Quit");
}

#[test]
fn the_keys_set_label_and_two_keys_narrow_then_jump() {
    let (mut d, mut editor) = at("a a a\n", &[0]);
    editor.set_jump_keys(Keys::new("qw").unwrap());
    send(&mut d, &mut editor, "M-j a RET w");
    assert_eq!(stage(&editor), Some(Stage::Labels), "narrowed");
    send(&mut d, &mut editor, "w");
    assert_eq!(cursors(&editor), [4]);
}

#[test]
fn m_g_f_labels_lines_and_a_digit_reads_a_line_number() {
    let (mut d, mut editor) = at("one\ntwo\nthree\n", &[0]);
    send(&mut d, &mut editor, "M-g f");
    assert_eq!(stage(&editor), Some(Stage::Labels));
    send(&mut d, &mut editor, "s");
    assert_eq!(cursors(&editor), [4]);
    send(&mut d, &mut editor, "M-g f 3");
    assert!(editor.jump().is_none());
    let prompt = editor.prompt().expect("goto-line's prompt");
    assert_eq!(prompt.label(), "Go to line: ");
    let answer = editor.buffer(prompt.buffer()).unwrap().text().to_string();
    assert_eq!(answer, "3");
    send(&mut d, &mut editor, "RET");
    assert_eq!(cursors(&editor), [8]);
}

#[test]
fn m_j_in_isearch_labels_its_matches_and_ends_it_at_the_one_chosen() {
    let (mut d, mut editor) = at("foo bar foo\n", &[0]);
    send(&mut d, &mut editor, "C-s f o o M-j");
    assert_eq!(stage(&editor), Some(Stage::Labels));
    assert!(editor.isearch().is_some());
    send(&mut d, &mut editor, "s");
    assert!(editor.isearch().is_none() && editor.jump().is_none());
    assert_eq!(cursors(&editor), [8]);
}

#[test]
fn a_jump_does_not_start_in_a_prompt() {
    let (mut d, mut editor) = at("a\n", &[0]);
    send(&mut d, &mut editor, "M-g g M-j");
    assert!(editor.jump().is_none());
}

#[test]
fn run_command_of_another_command_ends_a_jump() {
    let (mut d, mut editor) = at("ab\n", &[0]);
    send(&mut d, &mut editor, "M-j a");
    let (_, result) = d.run_command(&mut editor, "forward-char", None);
    assert_eq!(result, Ok(()));
    assert!(editor.jump().is_none());
    assert_eq!(cursors(&editor), [1]);
    send(&mut d, &mut editor, "M-j a");
    let (_, result) = d.run_command(&mut editor, "avy-show-labels", None);
    assert_eq!(result, Ok(()));
    assert_eq!(stage(&editor), Some(Stage::Labels));
}

#[test]
fn a_key_after_a_change_ended_the_labels_types_nothing_and_says_why() {
    let (mut d, mut editor) = at("ab abx\n", &[0]);
    send(&mut d, &mut editor, "M-j a RET");
    // Something the person did not do, such as a file still loading, changes the text.
    let id = editor.window().buffer();
    let edit = oso_core::buffer::Replacement {
        range: 6..6,
        text: "y",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    assert!(editor.isearch_follow_text());
    send(&mut d, &mut editor, "s");
    assert!(editor.jump().is_none());
    assert_eq!(text(&editor), "ab abxy\n", "the label's key typed nothing");
    assert_eq!(echo(&editor), "Jump ended: the text changed");
}

#[test]
fn del_at_the_labels_ends_the_jump_as_any_other_key_there() {
    let (mut d, mut editor) = at("ab ab\n", &[0]);
    send(&mut d, &mut editor, "M-j a RET DEL");
    assert!(editor.jump().is_none());
    assert_eq!(echo(&editor), "No such candidate");
    assert_eq!(text(&editor), "ab ab\n");
}

#[test]
fn a_digit_after_a_labels_first_key_does_not_read_a_line_number() {
    let (mut d, mut editor) = at(&"x\n".repeat(20), &[0]);
    editor.set_jump_keys(Keys::new("12").unwrap());
    send(&mut d, &mut editor, "M-g f 1");
    assert_eq!(stage(&editor), Some(Stage::Labels), "narrowed");
    send(&mut d, &mut editor, "7");
    assert!(editor.prompt().is_none(), "no goto-line");
    assert!(editor.jump().is_none());
    assert_eq!(echo(&editor), "No such candidate");
}
