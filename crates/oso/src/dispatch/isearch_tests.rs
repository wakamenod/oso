//! Scenario tests of isearch through the [`Dispatcher`] (ADR 0037, ADR 0082): its keymap in
//! place of the others, a key it does not bind ending the search and then running, and
//! `run-command` doing the same.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::editor::{Editor, Status};

use super::motion_tests::{at, cursors, send, text};
use crate::commands::kept_messages;

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

#[test]
fn c_s_types_into_the_search_and_ret_ends_it() {
    let (mut d, mut editor) = at("one two two\n", &[0]);
    assert!(send(&mut d, &mut editor, "C-s t w").redraw);
    assert_eq!(cursors(&editor), [6]);
    assert!(echo(&editor).starts_with("I-search: tw"));
    send(&mut d, &mut editor, "C-s");
    assert_eq!(cursors(&editor), [10]);
    send(&mut d, &mut editor, "RET");
    assert!(editor.isearch().is_none());
    assert_eq!(
        text(&editor),
        "one two two\n",
        "nothing was typed in the buffer"
    );
    assert_eq!(
        kept_messages(&editor).pop().as_deref(),
        Some("Mark saved where search started")
    );
}

#[test]
fn a_key_isearch_does_not_bind_ends_it_and_runs() {
    let (mut d, mut editor) = at("abc abc\n", &[0]);
    send(&mut d, &mut editor, "C-s b");
    assert_eq!(cursors(&editor), [2]);
    let effects = send(&mut d, &mut editor, "C-f");
    assert!(effects.redraw, "the highlights go");
    assert!(editor.isearch().is_none());
    assert_eq!(cursors(&editor), [3]);
    // A prefix key ends it too, and then waits.
    send(&mut d, &mut editor, "C-s a C-x");
    assert!(editor.isearch().is_none());
    assert!(!d.pending().is_empty());
    send(&mut d, &mut editor, "C-g");
}

#[test]
fn c_r_searches_backward_and_del_steps_back() {
    let (mut d, mut editor) = at("ab ab ab\n", &[8]);
    send(&mut d, &mut editor, "C-r a b");
    assert_eq!(cursors(&editor), [6]);
    send(&mut d, &mut editor, "C-r");
    assert_eq!(cursors(&editor), [3]);
    send(&mut d, &mut editor, "DEL");
    assert_eq!(cursors(&editor), [6]);
    send(&mut d, &mut editor, "DEL DEL");
    assert_eq!(cursors(&editor), [8]);
    assert_eq!(editor.isearch().unwrap().string(), "");
}

#[test]
fn c_g_on_a_failing_search_says_no_quit_and_keeps_the_line() {
    let (mut d, mut editor) = at("abc\n", &[0]);
    send(&mut d, &mut editor, "C-s a b x");
    assert!(echo(&editor).starts_with("Failing I-search: abx"));
    send(&mut d, &mut editor, "C-g");
    assert!(
        echo(&editor).starts_with("I-search: ab"),
        "{}",
        echo(&editor)
    );
    send(&mut d, &mut editor, "C-g");
    assert!(editor.isearch().is_none());
    assert_eq!(echo(&editor), "Quit");
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn text_from_the_socket_goes_into_the_search() {
    let (mut d, mut editor) = at("日本語\n", &[0]);
    send(&mut d, &mut editor, "C-s");
    d.insert_text(&mut editor, "本");
    assert_eq!(editor.isearch().unwrap().string(), "本");
    assert_eq!(cursors(&editor), [6]);
    assert_eq!(text(&editor), "日本語\n");
}

#[test]
fn run_command_ends_a_search_unless_it_is_one_of_its_own() {
    let (mut d, mut editor) = at("abc abc\n", &[0]);
    send(&mut d, &mut editor, "C-s b");
    let (_, result) = d.run_command(&mut editor, "isearch-repeat-forward", None);
    assert_eq!(result, Ok(()));
    assert_eq!(cursors(&editor), [6]);
    let (effects, result) = d.run_command(&mut editor, "forward-char", None);
    assert_eq!(result, Ok(()));
    assert!(effects.redraw);
    assert!(editor.isearch().is_none());
    assert_eq!(cursors(&editor), [7]);
}
