//! Scenario tests of query-replace through the [`Dispatcher`] (ADR 0037, ADR 0113): its two
//! prompts, its answers' keymap in place of the others, a key it does not bind ending it and
//! then running, `C-g`, and a read-only buffer refused before any prompt.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::editor::{Editor, Status};

use super::Dispatcher;
use super::motion_tests::{at, cursors, send, text};
use crate::commands::kept_messages;

/// Runs `!`'s slices to its end, as the host's turns do, and then the keys held meanwhile.
/// A slice ends after a few milliseconds of work, so a test reads the text only after this.
fn finish_bang(d: &mut Dispatcher, editor: &mut Editor) {
    while editor.replace_works() {
        editor.replace_continue();
    }
    d.run_held_during_all(editor);
}

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
    finish_bang(&mut d, &mut editor);
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

#[test]
fn c_m_percent_keeps_its_prompt_open_for_a_regexp_that_does_not_compile() {
    let (mut d, mut editor) = at("k=v\n", &[0]);
    send(&mut d, &mut editor, "C-M-% ( RET");
    let prompt = editor.prompt().expect("still open");
    assert_eq!(prompt.label(), "Query replace regexp: ");
    send(
        &mut d,
        &mut editor,
        "DEL ( \\ w ) = ( \\ w ) RET $ 2 = $ 1 RET !",
    );
    finish_bang(&mut d, &mut editor);
    assert_eq!(text(&editor), "v=k\n");
}

#[test]
fn input_during_bang_waits_for_it_and_c_g_stops_it() {
    let many = "ab ".repeat(3000);
    let (mut d, mut editor) = at(&many, &[0]);
    send(&mut d, &mut editor, "M-% a b RET c RET !");
    assert!(editor.replace_works(), "more than one slice");
    send(&mut d, &mut editor, "C-e");
    assert!(editor.replace_works(), "the key waits");
    finish_bang(&mut d, &mut editor);
    assert_eq!(text(&editor), "c ".repeat(3000));
    assert_eq!(cursors(&editor), [6000], "C-e ran after it");

    let (mut d, mut editor) = at(&many, &[0]);
    send(&mut d, &mut editor, "M-% a b RET c RET !");
    // A slice is a few milliseconds of work, at most 1000 matches: on a slow machine the
    // first may replace fewer than two, so it goes on until two are, still short of the end.
    while !text(&editor).starts_with("c c ") && editor.replace_works() {
        editor.replace_continue();
    }
    assert!(editor.replace_works(), "still replacing");
    send(&mut d, &mut editor, "C-g");
    assert!(editor.replace().is_none());
    assert!(text(&editor).starts_with("c c "), "kept what it replaced");
}
