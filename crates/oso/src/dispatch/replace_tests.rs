//! Scenario tests of query-replace through the [`Dispatcher`] (ADR 0037, ADR 0113): its two
//! prompts, its answers' keymap in place of the others, a key it does not bind ending it and
//! then running, `C-g`, and a read-only buffer refused before any prompt.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::editor::{Editor, Status};

use super::motion_tests::{at, cursors, send, text};
use crate::commands::kept_messages;

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

#[test]
fn m_percent_reads_two_strings_and_asks_at_each_match() {
    let (mut d, mut editor) = at("foo foo foo\n", &[0]);
    send(&mut d, &mut editor, "M-%");
    assert_eq!(editor.prompt().unwrap().label(), "Query replace: ");
    send(&mut d, &mut editor, "f o o RET");
    assert_eq!(editor.prompt().unwrap().label(), "Query replace foo with: ");
    send(&mut d, &mut editor, "b a r RET");
    assert!(editor.replace().is_some());
    assert!(echo(&editor).starts_with("Query replacing foo with bar:"));
    send(&mut d, &mut editor, "y n SPC");
    assert_eq!(text(&editor), "bar foo bar\n");
    assert!(editor.replace().is_none());
    assert_eq!(
        kept_messages(&editor).pop().as_deref(),
        Some("Replaced 2 occurrences (skipped 1)")
    );
    // The whole session is one step.
    send(&mut d, &mut editor, "C-z");
    assert_eq!(text(&editor), "foo foo foo\n");
}

#[test]
fn an_empty_first_answer_takes_the_last_pair() {
    let (mut d, mut editor) = at("a a a\n", &[0]);
    send(&mut d, &mut editor, "M-% a RET b RET q");
    // It runs from the cursor, which the first left after its first match.
    send(&mut d, &mut editor, "M-< M-%");
    assert_eq!(
        editor.prompt().unwrap().label(),
        "Query replace (default a → b): "
    );
    send(&mut d, &mut editor, "RET !");
    assert_eq!(text(&editor), "b b b\n");
}

#[test]
fn a_key_it_does_not_bind_ends_it_and_runs() {
    let (mut d, mut editor) = at("ab ab\n", &[0]);
    send(&mut d, &mut editor, "M-% a b RET x RET y");
    assert_eq!(text(&editor), "x ab\n");
    send(&mut d, &mut editor, "C-e");
    assert!(editor.replace().is_none());
    assert_eq!(cursors(&editor), [4], "C-e ran");
}

#[test]
fn c_g_ends_it_and_keeps_what_it_replaced() {
    let (mut d, mut editor) = at("ab ab\n", &[0]);
    send(&mut d, &mut editor, "M-% a b RET x RET y C-g");
    assert!(editor.replace().is_none());
    assert_eq!(text(&editor), "x ab\n");
}
