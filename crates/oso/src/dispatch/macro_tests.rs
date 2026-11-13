//! Scenario tests of keyboard macros through the [`Dispatcher`] (ADR 0037, ADR 0113): what a
//! recording takes, `e` again, counts, what stops a replay, `C-g`, input held during a replay,
//! and the replay's undo step.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::time::{Duration, Instant};

use oso_core::editor::Editor;
use oso_core::kill::{Fingerprint, FromClipboard};
use oso_core::minibuffer::Kind;
use oso_core::rope::Rope;

use super::Dispatcher;
use super::motion_tests::{at, drain, send, text};

/// Runs a replay to its end, as the host's turns would, with a search's worker jobs, which
/// a replay waits for.
fn replay(d: &mut Dispatcher, editor: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(10);
    while editor.macro_replaying() {
        d.replay(editor, until);
        drain(editor);
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
    send(&mut d, &mut editor, "C-x ( C-s x");
    drain(&mut editor);
    send(&mut d, &mut editor, "RET DEL C-x )");
    assert_eq!(text(&editor), "1 x2 x3\n");
    send(&mut d, &mut editor, "C-u 0 C-x e");
    replay(&mut d, &mut editor);
    assert_eq!(text(&editor), "1 2 3\n");
    assert!(editor.isearch().is_none() || editor.isearch().unwrap().failing());
}

#[test]
fn a_replay_stops_when_its_search_fails_on_the_worker() {
    // Past the last `x`, more text than a key searches itself (256 KiB): the search that
    // fails there goes on on the worker, which says so only after the key.
    let filler = "-".repeat(1024 * 1024);
    let (mut d, mut editor) = at(&format!("x1 x2 {filler}\n"), &[0]);
    send(&mut d, &mut editor, "C-x ( C-s x");
    drain(&mut editor);
    send(&mut d, &mut editor, "RET DEL C-x )");
    assert_eq!(text(&editor), format!("1 x2 {filler}\n"));
    send(&mut d, &mut editor, "C-u 0 C-x e");
    let until = Instant::now() + Duration::from_secs(10);
    let mut on_the_worker = false;
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
        on_the_worker |= editor.isearch().is_some_and(|search| search.searching());
        drain(&mut editor);
    }
    assert!(
        on_the_worker,
        "the replay waited for a search on the worker"
    );
    assert_eq!(
        text(&editor),
        format!("1 2 {filler}\n"),
        "the RET DEL after the failing search did not run"
    );
    assert!(editor.isearch().is_none_or(|search| search.failing()));
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

/// Runs a replay to its end, answering each clipboard read with `yanked`.
fn replay_yanking(d: &mut Dispatcher, editor: &mut Editor, yanked: &str) {
    let until = Instant::now() + Duration::from_secs(10);
    while editor.macro_replaying() {
        d.replay(editor, until);
        if d.take_clipboard_request().is_some() {
            d.clipboard_answered(editor, &clipboard(yanked));
        }
    }
}

fn clipboard(text: &str) -> FromClipboard {
    FromClipboard::Text {
        text: Rope::from(text),
        concealed: false,
        fingerprint: Fingerprint {
            len: text.len(),
            hash: 7,
        },
    }
}

#[test]
fn a_key_held_while_the_clipboard_is_read_is_recorded_once() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-x ( C-y");
    assert!(d.take_clipboard_request().is_some());
    send(&mut d, &mut editor, "a");
    d.clipboard_answered(&mut editor, &clipboard("Y"));
    send(&mut d, &mut editor, "C-x )");
    assert_eq!(text(&editor), "Ya\n");
    send(&mut d, &mut editor, "C-x e");
    replay_yanking(&mut d, &mut editor, "Y");
    assert_eq!(text(&editor), "YaYa\n");
}

#[test]
fn e_over_a_question_answers_it_and_runs_no_macro() {
    let (mut d, mut editor) = at("\n", &[0]);
    send(&mut d, &mut editor, "C-x ( a C-x ) C-x e");
    replay(&mut d, &mut editor);
    editor.ask("Exit?", "save-buffers-kill-terminal").unwrap();
    send(&mut d, &mut editor, "e");
    assert!(!editor.macro_replaying());
    assert!(editor.prompt().is_some(), "the question is still asked");
    assert_eq!(editor.tiled().1.text().to_string(), "aa\n");
}

#[test]
fn a_replay_feeds_nothing_once_a_question_or_a_passphrase_prompt_opened() {
    for passphrase in [false, true] {
        let (mut d, mut editor) = at("\n", &[0]);
        send(&mut d, &mut editor, "C-x ( y C-x ) C-u 0 C-x e");
        d.replay(&mut editor, Instant::now() + Duration::from_millis(1));
        let before = text(&editor);
        if passphrase {
            editor
                .open_prompt("Passphrase: ", Kind::Passphrase)
                .unwrap();
        } else {
            editor
                .ask("Let it in?", "save-buffers-kill-terminal")
                .unwrap();
        }
        d.replay(&mut editor, Instant::now() + Duration::from_secs(1));
        assert!(!editor.macro_replaying());
        assert_eq!(editor.tiled().1.text().to_string(), before);
        let typed = editor
            .buffer(editor.prompt().unwrap().buffer())
            .unwrap()
            .text()
            .to_string();
        assert_eq!(typed, "", "nothing went to the prompt");
    }
}

#[test]
fn c_g_during_bang_in_a_replay_stops_the_replay_first() {
    let (mut d, mut editor) = at(&"a ".repeat(3000), &[0]);
    send(&mut d, &mut editor, "C-x ( M-% a RET b RET !");
    while editor.replace_works() {
        editor.replace_continue();
    }
    d.run_held_during_all(&mut editor);
    send(&mut d, &mut editor, "C-x ) C-z M-<");
    super::undo_tests::finish(&mut d, &mut editor);
    assert_eq!(text(&editor), "a ".repeat(3000));
    send(&mut d, &mut editor, "C-x e");
    d.replay(&mut editor, Instant::now() + Duration::from_secs(1));
    assert!(editor.replace_works(), "`!` runs its slices");
    send(&mut d, &mut editor, "C-g");
    assert!(!editor.macro_replaying(), "C-g stopped the replay");
    assert!(editor.replace_works(), "and not yet `!`");
    send(&mut d, &mut editor, "C-g");
    assert!(!editor.replace_works());
}
