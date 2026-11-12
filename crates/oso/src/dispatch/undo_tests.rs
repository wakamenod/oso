//! Scenario tests of undoing a large step through the [`Dispatcher`] (ADR 0119): the command
//! begins it and the host goes on with it a slice a turn, input waits meanwhile and runs after,
//! and `C-g` cancels it, dropping that input.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::time::{Duration, Instant};

use oso_core::editor::{Editor, Status};

use super::Dispatcher;
use super::motion_tests::{at, cursors, send, text};
use crate::commands::kept_messages;

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

/// Goes on with the undo under way until it ends, as the host does each turn.
pub(super) fn finish(d: &mut Dispatcher, editor: &mut Editor) {
    while editor.crossing().is_some() {
        d.go_on_crossing(editor);
    }
}

/// A buffer whose last step replaced 10,000 matches: more than two slices' work to undo.
fn replaced() -> (Dispatcher, Editor) {
    let (mut d, mut editor) = at(&"ab ".repeat(10_000), &[0]);
    send(&mut d, &mut editor, "M-% a b RET c RET !");
    while editor.replace_works() {
        editor.replace_continue();
    }
    d.run_held_during_all(&mut editor);
    assert_eq!(text(&editor), "c ".repeat(10_000));
    (d, editor)
}

#[test]
fn keys_wait_while_a_large_undo_goes_on_and_run_after_it_in_order() {
    let (mut d, mut editor) = replaced();
    send(&mut d, &mut editor, "C-z");
    assert!(editor.crossing().is_some(), "more than one slice");
    assert_eq!(text(&editor), "c ".repeat(10_000), "nothing undone yet");
    send(&mut d, &mut editor, "M-> x");
    assert!(editor.crossing().is_some(), "the keys wait");
    // A close request waits with them, and asks nothing yet.
    assert_eq!(d.close_requested(&mut editor), Default::default());
    assert!(editor.prompt().is_none());
    d.go_on_crossing(&mut editor);
    assert!(echo(&editor).starts_with("Undoing… "), "{}", echo(&editor));
    finish(&mut d, &mut editor);
    let (window, buffer) = editor.tiled();
    assert_eq!(
        buffer.text().to_string(),
        format!("{}x", "ab ".repeat(10_000))
    );
    assert_eq!(window.selections.primary().cursor, 30_001, "M->, then x");
    assert!(editor.prompt().is_some(), "then the close request asks");
}

#[test]
fn c_g_cancels_a_large_undo_and_drops_the_keys_that_waited() {
    let (mut d, mut editor) = replaced();
    let before = cursors(&editor);
    send(&mut d, &mut editor, "C-z");
    send(&mut d, &mut editor, "M-> x C-g");
    assert!(editor.crossing().is_none());
    assert!(d.take_crossing_cancelled());
    assert!(!d.take_crossing_cancelled(), "said once");
    assert_eq!(text(&editor), "c ".repeat(10_000), "nothing undone");
    assert_eq!(cursors(&editor), before, "nor moved");
    assert_eq!(
        kept_messages(&editor).pop().as_deref(),
        Some("Undo cancelled")
    );
    // The step can still be undone, and a redo cancelled says so.
    send(&mut d, &mut editor, "C-z");
    finish(&mut d, &mut editor);
    assert_eq!(text(&editor), "ab ".repeat(10_000));
    send(&mut d, &mut editor, "C-M-z C-g");
    assert_eq!(
        kept_messages(&editor).pop().as_deref(),
        Some("Redo cancelled")
    );
    assert_eq!(text(&editor), "ab ".repeat(10_000));
}

#[test]
fn a_replay_waits_for_an_undo_its_key_began() {
    let (mut d, mut editor) = replaced();
    send(&mut d, &mut editor, "C-z");
    finish(&mut d, &mut editor);
    // Recorded as they run: the redo first, then the keys that waited for it.
    send(&mut d, &mut editor, "C-x ( C-M-z M-> x C-x )");
    finish(&mut d, &mut editor);
    let done = format!("{}x", "c ".repeat(10_000));
    assert_eq!(text(&editor), done);
    for _ in 0..2 {
        send(&mut d, &mut editor, "C-z");
        finish(&mut d, &mut editor);
    }
    assert_eq!(text(&editor), "ab ".repeat(10_000));
    send(&mut d, &mut editor, "C-x e");
    while editor.macro_replaying() {
        d.replay(&mut editor, Instant::now() + Duration::from_secs(1));
        if editor.crossing().is_some() {
            assert_eq!(text(&editor), "ab ".repeat(10_000), "the replay waits");
            finish(&mut d, &mut editor);
        }
    }
    assert_eq!(text(&editor), done);
}
