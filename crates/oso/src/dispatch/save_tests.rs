//! Scenario tests of saving and of the exit guard, with their keys, through the
//! [`Dispatcher`] (ADR 0037, ADR 0059). The save runs where the test says, with no
//! thread, so what happens while one runs is deterministic.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::fs;
use std::path::Path;

use oso_core::buffer::Buffer;
use oso_core::command::Effects;
use oso_core::editor::{Editor, ExitAction};
use oso_core::key::KeySequence;
use oso_core::window::ViewSize;

use super::Dispatcher;
use crate::saver;
use crate::saver::tests::{Scratch, visit};

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) -> Effects {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0)
}

fn visiting(path: &Path) -> Editor {
    let mut editor = visit(path);
    editor.set_view_size(ViewSize::new(10, 20));
    editor
}

/// Runs the saves that are waiting, as their threads would.
fn finish_saves(editor: &mut Editor) {
    for job in editor.take_save_jobs() {
        saver::apply(editor, saver::run(job));
    }
}

fn last_message(editor: &Editor) -> &str {
    editor.messages().next_back().unwrap_or("")
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn modified(editor: &Editor) -> bool {
    editor.current().1.is_modified()
}

#[test]
fn the_keys_save_and_the_message_says_so() {
    let scratch = Scratch::new("dispatch-keys");
    let path = scratch.file("notes.txt", b"old\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    assert!(modified(&editor), "until the write has ended");
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
    finish_saves(&mut editor);
    assert!(!modified(&editor));
    assert!(!editor.current().1.is_saving());
    assert_eq!(fs::read(&path).unwrap(), b"xold\n");
    assert_eq!(last_message(&editor), format!("Wrote {}", path.display()));
}

#[test]
fn a_new_file_is_created_on_the_first_save() {
    let scratch = Scratch::new("dispatch-new");
    let path = scratch.path().join("new.txt");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "a b C-x C-s");
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"ab\n");
    assert!(!modified(&editor));

    send(&mut dispatcher, &mut editor, "C-e c C-x C-s");
    finish_saves(&mut editor);
    assert_eq!(
        fs::read(&path).unwrap(),
        b"abc\n",
        "no conflict with itself"
    );
}

#[test]
fn an_unmodified_buffer_says_so_and_writes_nothing() {
    let scratch = Scratch::new("dispatch-unmodified");
    let path = scratch.file("notes.txt", b"no final newline");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    let effects = send(&mut dispatcher, &mut editor, "C-x C-s");
    assert!(!effects.redraw);
    assert!(editor.take_save_jobs().is_empty());
    assert_eq!(last_message(&editor), "(No changes need to be saved)");
    assert_eq!(text(&editor), "no final newline");
    assert_eq!(fs::read(&path).unwrap(), b"no final newline");
}

#[test]
fn a_new_file_left_untouched_is_not_created() {
    let scratch = Scratch::new("dispatch-untouched");
    let path = scratch.path().join("new.txt");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "C-x C-s");
    assert!(editor.take_save_jobs().is_empty());
    assert!(!path.exists());
}

#[test]
fn a_buffer_with_no_file_says_so() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "C-x C-s");
    assert_eq!(last_message(&editor), "(No changes need to be saved)");
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    assert!(editor.take_save_jobs().is_empty());
    assert_eq!(last_message(&editor), "Buffer has no file name");
    assert_eq!(text(&editor), "x", "no final newline either");
    let (effects, result) = dispatcher.run_command(&mut editor, "save-buffer", None);
    assert_eq!(effects, Effects::default());
    assert!(result.unwrap_err().contains("no file name"));
}

#[test]
fn read_only_buffers_are_refused_and_their_files_stay() {
    let scratch = Scratch::new("dispatch-read-only");
    let cases: [(&str, &[u8], &str); 2] = [
        ("binary.bin", b"a\0b", "binary"),
        ("invalid.txt", b"a\xffb", "invalid UTF-8"),
    ];
    for (name, content, said) in cases {
        let path = scratch.file(name, content);
        let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
        send(&mut dispatcher, &mut editor, "C-x C-s");
        assert!(editor.take_save_jobs().is_empty());
        assert!(
            last_message(&editor).contains(said),
            "{}",
            last_message(&editor)
        );
        assert_eq!(fs::read(&path).unwrap(), content);
    }
    let path = scratch.path().join("loading.txt");
    let mut editor = Editor::with_buffer(Buffer::visiting(path));
    send(&mut Dispatcher::new(), &mut editor, "C-x C-s");
    assert!(editor.take_save_jobs().is_empty());
    assert!(last_message(&editor).contains("loading"));
}

#[test]
fn the_final_newline_is_one_undo_step_and_the_cursor_stays_before_it() {
    let scratch = Scratch::new("dispatch-newline");
    let path = scratch.file("notes.txt", b"abc");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "C-e x");
    let effects = send(&mut dispatcher, &mut editor, "C-x C-s");
    assert!(effects.redraw, "a new last line");
    assert_eq!(text(&editor), "abcx\n", "the buffer matches the disk");
    assert_eq!(editor.window().selections.primary().cursor, 4);
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"abcx\n");

    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(text(&editor), "abcx", "only the line break");
    assert!(modified(&editor));
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(text(&editor), "abc");
    assert!(modified(&editor), "the loaded state is no longer on disk");
}

#[test]
fn typing_after_a_save_continues_the_line() {
    let scratch = Scratch::new("dispatch-continue");
    let path = scratch.path().join("new.txt");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "a C-x C-s");
    finish_saves(&mut editor);
    send(&mut dispatcher, &mut editor, "b");
    assert_eq!(text(&editor), "ab\n");
}

#[test]
fn undoing_and_redoing_past_the_saved_state_tells_it_apart() {
    let scratch = Scratch::new("dispatch-undo");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    finish_saves(&mut editor);
    assert!(!modified(&editor));
    send(&mut dispatcher, &mut editor, "C-z");
    assert!(modified(&editor));
    send(&mut dispatcher, &mut editor, "C-M-z");
    assert!(!modified(&editor), "back at the saved state");
    send(&mut dispatcher, &mut editor, "y");
    assert!(modified(&editor));
    send(&mut dispatcher, &mut editor, "C-z");
    assert!(!modified(&editor));
}

#[test]
fn edits_made_while_a_save_runs_stay_modified() {
    let scratch = Scratch::new("dispatch-while-saving");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s y");
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"xabc\n", "the state when asked");
    assert_eq!(text(&editor), "xyabc\n");
    assert!(modified(&editor));
    send(&mut dispatcher, &mut editor, "C-x C-s");
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"xyabc\n");
    assert!(!modified(&editor));
}

#[test]
fn undoing_while_a_save_runs_leaves_the_buffer_modified_after_it() {
    let scratch = Scratch::new("dispatch-undo-while-saving");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s C-z");
    assert_eq!(text(&editor), "abc\n");
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"xabc\n");
    assert!(modified(&editor), "the disk differs from the buffer");
}

#[test]
fn a_second_save_while_one_runs_is_refused_and_says_so() {
    let scratch = Scratch::new("dispatch-second");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    let first = editor.take_save_jobs();
    send(&mut dispatcher, &mut editor, "y C-x C-s");
    assert!(editor.take_save_jobs().is_empty());
    assert!(last_message(&editor).contains("already being saved"));
    for job in first {
        saver::apply(&mut editor, saver::run(job));
    }
    assert_eq!(fs::read(&path).unwrap(), b"xabc\n");
    assert!(modified(&editor));
}

#[test]
fn a_conflict_keeps_the_file_and_the_edits_each_time_it_is_tried() {
    let scratch = Scratch::new("dispatch-conflict");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x");
    fs::write(&path, b"another program\n").unwrap();
    for _ in 0..2 {
        send(&mut dispatcher, &mut editor, "C-x C-s");
        finish_saves(&mut editor);
        assert!(modified(&editor));
        assert_eq!(text(&editor), "xabc\n");
        assert!(last_message(&editor).contains("changed on disk"));
        assert_eq!(fs::read(&path).unwrap(), b"another program\n");
    }
}

#[test]
fn a_save_failure_says_why_and_leaves_the_buffer_modified() {
    let scratch = Scratch::new("dispatch-failure");
    let path = scratch.path().join("no-such-directory").join("new.txt");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    finish_saves(&mut editor);
    assert!(modified(&editor));
    assert!(!editor.current().1.is_saving());
    assert!(last_message(&editor).starts_with("Cannot save"));
}

const QUIT: &str = "C-x C-c";

#[test]
fn exit_goes_at_once_with_nothing_at_risk() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    assert!(send(&mut dispatcher, &mut editor, QUIT).exit);
    assert_eq!(editor.confirm_exit(ExitAction::CloseWindow), Ok(()));
}

#[test]
fn exit_is_refused_once_with_a_modified_buffer_and_repeating_confirms() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert_eq!(
        last_message(&editor),
        "1 modified buffer; repeat to exit anyway"
    );
    assert!(send(&mut dispatcher, &mut editor, QUIT).exit);
}

#[test]
fn any_other_key_resets_the_guard_but_the_quit_keys_prefix_does_not() {
    let others = [
        "C-f",       // a command
        "C-g",       // a command that cancels
        "a",         // text
        "<f35>",     // unbound
        "C-x <f35>", // unbound under the prefix
        "C-x C-g",   // a cancelled prefix
    ];
    for other in others {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
        send(&mut dispatcher, &mut editor, "x");
        assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
        send(&mut dispatcher, &mut editor, other);
        assert!(
            !send(&mut dispatcher, &mut editor, QUIT).exit,
            "after {other}"
        );
        assert!(
            send(&mut dispatcher, &mut editor, QUIT).exit,
            "after {other}"
        );
    }
}

#[test]
fn a_command_that_fails_also_resets_the_guard() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    // At the end of the buffer, so this signals.
    send(&mut dispatcher, &mut editor, "C-f");
    assert!(last_message(&editor).contains("End of buffer"));
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert!(send(&mut dispatcher, &mut editor, QUIT).exit);
}

#[test]
fn text_from_an_input_method_resets_the_guard() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    dispatcher.insert_text(&mut editor, "日本語");
    assert_eq!(editor.exit_attempt(), None);
}

#[test]
fn the_close_button_and_the_quit_keys_do_not_confirm_each_other() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert!(editor.confirm_exit(ExitAction::CloseWindow).is_err());
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert!(send(&mut dispatcher, &mut editor, QUIT).exit);
}

#[test]
fn a_key_after_the_close_button_resets_it() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    assert!(editor.confirm_exit(ExitAction::CloseWindow).is_err());
    send(&mut dispatcher, &mut editor, "C-f");
    assert!(editor.confirm_exit(ExitAction::CloseWindow).is_err());
    assert_eq!(editor.confirm_exit(ExitAction::CloseWindow), Ok(()));
}

#[test]
fn exit_counts_a_running_save_and_a_finished_one_lets_it_go() {
    let scratch = Scratch::new("dispatch-exit-saving");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert_eq!(
        last_message(&editor),
        "1 modified buffer and 1 save running; repeat to exit anyway"
    );
    finish_saves(&mut editor);
    send(&mut dispatcher, &mut editor, "C-f");
    assert!(
        send(&mut dispatcher, &mut editor, QUIT).exit,
        "nothing at risk"
    );
}

#[test]
fn a_save_that_fails_after_the_exit_was_refused_needs_a_new_refusal() {
    let scratch = Scratch::new("dispatch-exit-failed");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    fs::write(&path, b"another program\n").unwrap();
    finish_saves(&mut editor);
    assert!(
        !send(&mut dispatcher, &mut editor, QUIT).exit,
        "it did not save"
    );
    assert!(send(&mut dispatcher, &mut editor, QUIT).exit);
}

#[test]
fn run_command_reports_a_refused_exit() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    let (effects, result) = dispatcher.run_command(&mut editor, "save-buffers-kill-terminal", None);
    assert!(!effects.exit);
    assert!(result.unwrap_err().contains("1 modified buffer"));
    let (effects, result) = dispatcher.run_command(&mut editor, "save-buffers-kill-terminal", None);
    assert!(effects.exit);
    assert_eq!(result, Ok(()));
}
