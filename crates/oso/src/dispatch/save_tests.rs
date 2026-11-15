//! Scenario tests of saving and of the exit guard, with their keys, through the
//! [`Dispatcher`] (ADR 0037, ADR 0059). The save runs where the test says, with no
//! thread, so what happens while one runs is deterministic.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::fs;
use std::path::Path;

use oso_core::buffer::Buffer;
use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::window::ViewSize;

use super::Dispatcher;
use crate::commands::kept_messages;
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

fn last_message(editor: &Editor) -> String {
    kept_messages(editor).pop().unwrap_or_default()
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
    finish_saves(&mut editor);
    assert!(!editor.current().1.is_saving());
    assert_eq!(last_message(&editor), "(No changes need to be saved)");
    assert_eq!(text(&editor), "no final newline");
    assert_eq!(fs::read(&path).unwrap(), b"no final newline");
}

#[test]
fn an_unmodified_buffer_whose_file_was_deleted_is_written_again() {
    let scratch = Scratch::new("dispatch-deleted");
    let path = scratch.file("notes.txt", b"the only copy\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    fs::remove_file(&path).unwrap();
    send(&mut dispatcher, &mut editor, "C-x C-s");
    assert!(editor.current().1.is_saving());
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"the only copy\n");
    assert_eq!(last_message(&editor), format!("Wrote {}", path.display()));
    assert!(!modified(&editor));
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
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(&[path]));
    let mut editor = Editor::with_buffer(buffer);
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
fn with_trim_whitespace_a_save_cleans_the_lines_edited_and_undoes_as_one_step() {
    let scratch = Scratch::new("dispatch-trim");
    let path = scratch.file("notes.txt", b"keep  \nedit");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    editor.set_trim_whitespace(true);
    // A space typed on the second line, then the cursor back to the first.
    send(&mut dispatcher, &mut editor, "C-n C-e SPC SPC M-<");
    send(&mut dispatcher, &mut editor, "C-x C-s");
    assert_eq!(
        text(&editor),
        "keep  \nedit\n",
        "the line not edited keeps its spaces"
    );
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"keep  \nedit\n");
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(
        text(&editor),
        "keep  \nedit  ",
        "the cleanup and the line break together"
    );
}

#[test]
fn delete_trailing_whitespace_cleans_the_whole_buffer() {
    let scratch = Scratch::new("dispatch-delete-trailing");
    let path = scratch.file("notes.txt", b"a  \nb\t\n\n\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    let name: Vec<String> = "delete-trailing-whitespace"
        .chars()
        .map(String::from)
        .collect();
    send(
        &mut dispatcher,
        &mut editor,
        &format!("M-x {} RET", name.join(" ")),
    );
    assert_eq!(text(&editor), "a\nb\n");
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(text(&editor), "a  \nb\t\n\n\n");
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
    // The second save asks first, and the answer here is no.
    for keys in ["C-x C-s", "C-x C-s n"] {
        send(&mut dispatcher, &mut editor, keys);
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
    assert!(last_message(&editor).starts_with("Not saved"));
}

const QUIT: &str = "C-x C-c";

/// The question the innermost prompt asks, if it is one.
fn question(editor: &Editor) -> Option<&str> {
    editor
        .prompt()
        .filter(|prompt| prompt.kind().is_question())
        .map(|prompt| prompt.label())
}

#[test]
fn exit_goes_at_once_with_nothing_at_risk() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    assert!(send(&mut dispatcher, &mut editor, QUIT).exit);
    assert!(editor.prompt().is_none());
}

#[test]
fn exit_with_a_modified_buffer_asks_and_y_exits() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert_eq!(
        question(&editor),
        Some("1 modified buffer; exit without saving? (y or n)")
    );
    assert!(send(&mut dispatcher, &mut editor, "y").exit);
}

#[test]
fn n_or_c_g_stays_and_the_next_exit_asks_again() {
    for answer in ["n", "C-g"] {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
        send(&mut dispatcher, &mut editor, "x");
        send(&mut dispatcher, &mut editor, QUIT);
        assert!(!send(&mut dispatcher, &mut editor, answer).exit, "{answer}");
        assert!(editor.prompt().is_none(), "{answer} closes the question");
        assert!(
            !send(&mut dispatcher, &mut editor, QUIT).exit,
            "after {answer}"
        );
        assert!(question(&editor).is_some(), "it asks again after {answer}");
    }
}

#[test]
fn only_y_and_n_answer_and_any_other_key_says_so() {
    for other in ["SPC", "RET", "DEL", "a", "C-f", "C-x", "<f35>"] {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
        send(&mut dispatcher, &mut editor, "x");
        send(&mut dispatcher, &mut editor, QUIT);
        assert!(!send(&mut dispatcher, &mut editor, other).exit, "{other}");
        assert!(dispatcher.pending().is_empty(), "{other} leaves no prefix");
        let prompt = editor.prompt().expect("still asking");
        assert_eq!(prompt.message(), Some("Answer y or n"), "{other}");
        assert_eq!(editor.current().1.text().len(), 0, "{other} typed nothing");
    }
}

#[test]
fn text_from_an_input_method_answers_no_question() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    send(&mut dispatcher, &mut editor, QUIT);
    assert!(!dispatcher.insert_text(&mut editor, "y").exit);
    assert!(question(&editor).is_some());
}

#[test]
fn exit_counts_a_running_save_and_a_finished_one_lets_it_go() {
    let scratch = Scratch::new("dispatch-exit-saving");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert_eq!(
        question(&editor),
        Some("1 modified buffer and 1 save running; exit without saving? (y or n)")
    );
    finish_saves(&mut editor);
    send(&mut dispatcher, &mut editor, "C-g");
    assert!(
        send(&mut dispatcher, &mut editor, QUIT).exit,
        "nothing at risk"
    );
}

#[test]
fn a_save_that_fails_while_the_question_waits_makes_it_ask_again() {
    let scratch = Scratch::new("dispatch-exit-failed");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    fs::write(&path, b"another program\n").unwrap();
    finish_saves(&mut editor);
    assert!(
        !send(&mut dispatcher, &mut editor, "y").exit,
        "it did not save, which the question never said"
    );
    assert!(question(&editor).is_some());
    assert!(send(&mut dispatcher, &mut editor, "y").exit);
}

#[test]
fn run_command_asks_too_and_the_answer_exits() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    let (effects, result) = dispatcher.run_command(&mut editor, "save-buffers-kill-terminal", None);
    assert!(!effects.exit);
    assert_eq!(result, Ok(()));
    assert!(question(&editor).is_some_and(|q| q.starts_with("1 modified buffer")));
    assert!(send(&mut dispatcher, &mut editor, "y").exit);
}

#[test]
fn a_file_changed_on_disk_is_saved_over_once_agreed() {
    let scratch = Scratch::new("dispatch-save-over");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    fs::write(&path, b"another program\n").unwrap();
    finish_saves(&mut editor);
    assert!(last_message(&editor).contains("C-x C-s asks to save over it"));
    send(&mut dispatcher, &mut editor, "C-x C-s");
    assert_eq!(
        question(&editor),
        Some("Save over notes.txt? It changed on disk since it was read. (y or n)")
    );
    send(&mut dispatcher, &mut editor, "n");
    assert_eq!(fs::read(&path).unwrap(), b"another program\n", "n keeps it");
    send(&mut dispatcher, &mut editor, "C-x C-s y");
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"xabc\n");
    assert!(!modified(&editor));
}

#[test]
fn a_file_changed_again_after_the_question_is_kept_and_asked_about_again() {
    let scratch = Scratch::new("dispatch-save-over-again");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    fs::write(&path, b"another program\n").unwrap();
    finish_saves(&mut editor);
    send(&mut dispatcher, &mut editor, "C-x C-s");
    // The other program writes again, longer, before the answer.
    fs::write(&path, b"another program, again\n").unwrap();
    send(&mut dispatcher, &mut editor, "y");
    finish_saves(&mut editor);
    assert_eq!(fs::read(&path).unwrap(), b"another program, again\n");
    assert!(modified(&editor));
    send(&mut dispatcher, &mut editor, "C-x C-s");
    assert!(question(&editor).is_some());
}

mod leaving;

#[test]
fn revert_asks_before_it_discards_edits() {
    let scratch = Scratch::new("dispatch-revert");
    let path = scratch.file("notes.txt", b"old\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    let id = editor.window().buffer();
    send(&mut dispatcher, &mut editor, "C-x x g");
    let read = editor.current().1.content_id();
    assert_eq!(
        editor.take_revert_requests(),
        vec![(id, read)],
        "nothing to lose"
    );

    send(&mut dispatcher, &mut editor, "x C-x x g");
    assert!(editor.prompt().is_some());
    send(&mut dispatcher, &mut editor, "n");
    assert!(editor.take_revert_requests().is_empty());
    assert_eq!(text(&editor), "xold\n");

    send(&mut dispatcher, &mut editor, "C-x x g y");
    let edited = editor.current().1.content_id();
    assert_eq!(editor.take_revert_requests(), vec![(id, edited)]);
}

#[test]
fn revert_needs_a_file() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "C-x x g");
    assert!(editor.take_revert_requests().is_empty());
    assert_eq!(
        last_message(&editor),
        "Buffer does not seem to be associated with any file"
    );
}
