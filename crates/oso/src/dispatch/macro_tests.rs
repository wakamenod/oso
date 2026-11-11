//! Scenario tests of keyboard macros through the [`Dispatcher`] (ADR 0037, ADR 0113): what a
//! recording takes, `e` again, counts, what stops a replay, `C-g`, input held during a replay,
//! and the replay's undo step.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::time::{Duration, Instant};

use oso_core::editor::Editor;

use super::Dispatcher;
use super::motion_tests::{at, send, text};

/// Runs a replay to its end, as the host's turns would.
fn replay(d: &mut Dispatcher, editor: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(10);
    while editor.macro_replaying() {
        d.replay(editor, until);
    }
}

#[test]
fn a_macro_replays_what_was_typed_and_e_runs_it_again() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-x ( a b C-x )");
    assert_eq!(text(&editor), "ab\n");
    send(&mut d, &mut editor, "C-x e");
    replay(&mut d, &mut editor);
    assert_eq!(text(&editor), "abab\n");
    send(&mut d, &mut editor, "e");
    replay(&mut d, &mut editor);
    assert_eq!(text(&editor), "ababab\n");
    send(&mut d, &mut editor, "e");
    replay(&mut d, &mut editor);
    send(&mut d, &mut editor, "C-f e");
    assert_eq!(
        text(&editor),
        "abababab\ne",
        "e types once another key came between"
    );
}

#[test]
fn a_count_runs_it_that_many_times_and_undoes_as_one_step() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-x ( x C-x ) C-u 3 C-x e");
    replay(&mut d, &mut editor);
    assert_eq!(text(&editor), "xxxx\n");
    send(&mut d, &mut editor, "C-z");
    assert_eq!(text(&editor), "x\n", "the replay's three at once");
}

#[test]
fn zero_runs_it_until_an_error_stops_it() {
    let (mut d, mut editor) = at("a\nb\nc\n", &[0]);
    send(&mut d, &mut editor, "C-x ( C-a - C-n C-x )");
    send(&mut d, &mut editor, "C-u 0 C-x e");
    replay(&mut d, &mut editor);
    assert_eq!(text(&editor), "-a\n-b\n-c\n-");
    assert!(!editor.macro_replaying());
}

#[test]
fn c_g_with_nothing_else_to_do_drops_the_recording() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-x ( a C-x )");
    send(&mut d, &mut editor, "C-x ( b C-g");
    assert!(!editor.macro_recording());
    send(&mut d, &mut editor, "C-x e");
    replay(&mut d, &mut editor);
    assert_eq!(text(&editor), "aba\n", "the last macro stayed");
}

#[test]
fn input_during_a_replay_waits_for_it_and_c_g_stops_it() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-x ( a C-x )");
    send(&mut d, &mut editor, "C-u 0 C-x e");
    // Typed before the replay's first turn: it waits.
    send(&mut d, &mut editor, "z");
    let until = Instant::now() + Duration::from_millis(1);
    d.replay(&mut editor, until);
    assert!(editor.macro_replaying());
    send(&mut d, &mut editor, "C-g");
    assert!(!editor.macro_replaying());
    assert!(text(&editor).starts_with("aa"), "{}", text(&editor));
    assert!(!text(&editor).contains('z'), "dropped with the replay");
}

#[test]
fn a_replay_runs_a_search_and_stops_where_it_fails() {
    let (mut d, mut editor) = at("x1 x2 x3\n", &[0]);
    send(&mut d, &mut editor, "C-x ( C-s x RET DEL C-x )");
    assert_eq!(text(&editor), "1 x2 x3\n");
    send(&mut d, &mut editor, "C-u 0 C-x e");
    replay(&mut d, &mut editor);
    assert_eq!(text(&editor), "1 2 3\n");
    assert!(editor.isearch().is_none() || editor.isearch().unwrap().failing());
}

#[test]
fn a_question_stops_the_replay_and_waits_for_the_person() {
    let (mut d, mut editor) = at("\n", &[0]);
    // Recorded: a key, the exit's question, and its `n`.
    send(&mut d, &mut editor, "C-x ( a C-x C-c n C-x )");
    assert!(editor.prompt().is_none());
    send(&mut d, &mut editor, "C-x e");
    replay(&mut d, &mut editor);
    assert!(!editor.macro_replaying());
    let question = editor.prompt().expect("left open");
    assert!(question.kind().is_question());
    assert_eq!(
        editor.tiled().1.text().to_string(),
        "aa\n",
        "the recorded n answered nothing"
    );
}

#[cfg(feature = "dev")]
#[test]
fn a_passphrase_prompt_drops_the_recording() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-x ( a <f34>");
    assert!(!editor.macro_recording(), "nothing typed there is kept");
}
