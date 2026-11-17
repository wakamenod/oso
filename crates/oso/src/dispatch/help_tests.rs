//! `view-lossage` and `describe-key` through the [`Dispatcher`] (ADR 0150): what `*Lossage*`
//! keeps of each key, what it hides, and what `describe-key` reads and says without running.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::path::PathBuf;

use oso_core::buffer::Buffer;
use oso_core::editor::{Editor, Status};

use super::Dispatcher;
use oso_core::kill::FromClipboard;
use oso_core::minibuffer::Kind;

use super::terminal_tests::{in_terminal, keys, send, sent};

fn on_notes() -> (Dispatcher, Editor) {
    let editor = Editor::with_buffer(Buffer::from_text("notes.txt", "abc\n"));
    (Dispatcher::new(), editor)
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

fn shown(editor: &Editor) -> (String, String) {
    let buffer = editor.current().1;
    (buffer.name().to_string(), buffer.text().to_string())
}

/// The lines of `*Lossage*` once `C-h l` shows it, without the line of `C-h l` itself.
fn lossage(dispatcher: &mut Dispatcher, editor: &mut Editor) -> Vec<String> {
    send(dispatcher, editor, "C-h l");
    let (name, text) = shown(editor);
    assert_eq!(name, "*Lossage*");
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let (last, kept) = lines.split_last().unwrap();
    assert_eq!(last, "l                   ;; view-lossage");
    assert_eq!(kept.last().map(String::as_str), Some("C-h"));
    kept[..kept.len() - 1].to_vec()
}

#[test]
fn the_lossage_shows_each_key_with_what_it_ran_and_the_cursor_after_the_newest() {
    let (mut dispatcher, mut editor) = on_notes();
    send(&mut dispatcher, &mut editor, "C-f a C-x C-z M-<");
    assert_eq!(
        lossage(&mut dispatcher, &mut editor),
        [
            "C-f                 ;; forward-char",
            "a                   ;; self-insert-command",
            "C-x",
            "C-z                 ;; undefined",
            "M-<                 ;; beginning-of-buffer",
        ]
    );
    let buffer = editor.current().1;
    assert!(buffer.is_internal() && buffer.withheld_from_agents());
    let end = buffer.text().len();
    assert_eq!(editor.window().selections.primary().cursor, end);
}

#[test]
fn the_lossage_keeps_the_newest_300_keys() {
    let (mut dispatcher, mut editor) = on_notes();
    send(&mut dispatcher, &mut editor, "C-a");
    for _ in 0..299 {
        send(&mut dispatcher, &mut editor, "C-f");
    }
    send(&mut dispatcher, &mut editor, "C-e");
    // 301 keys, then `C-h l` itself: the oldest three are gone.
    send(&mut dispatcher, &mut editor, "C-h l");
    let (_, text) = shown(&editor);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 300);
    assert_eq!(lines[0], "C-f                 ;; forward-char");
    assert_eq!(
        lines[297],
        "C-e                 ;; mwim-end-of-code-or-line"
    );
}

#[test]
fn keys_in_a_buffer_that_may_hold_secrets_are_hidden_with_what_they_ran() {
    let path = PathBuf::from("/test/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(&[path]));
    secret.finish_empty();
    let mut editor = Editor::with_buffer(secret);
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    let mut dispatcher = Dispatcher::new();
    send(&mut dispatcher, &mut editor, "p w C-a");
    editor.show(notes);
    send(&mut dispatcher, &mut editor, "x");
    assert_eq!(
        lossage(&mut dispatcher, &mut editor),
        ["<hidden>", "x                   ;; self-insert-command"]
    );
}

#[test]
fn characters_sent_to_a_terminal_are_hidden_but_named_and_modified_keys_show() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "l s RET C-a M-b");
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(notes);
    assert_eq!(
        lossage(&mut dispatcher, &mut editor),
        [
            // A run of hidden keys is one line, which tells no length.
            "<hidden>            ;; sent to the terminal",
            "RET                 ;; sent to the terminal",
            "C-a                 ;; sent to the terminal",
            "M-b                 ;; sent to the terminal",
        ]
    );
}

#[test]
fn a_key_with_no_emacs_name_shows_as_such() {
    let (mut dispatcher, mut editor) = on_notes();
    dispatcher.note_unnamed_key(&editor);
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(
        lossage(&mut dispatcher, &mut editor),
        ["<no Emacs name>", "C-f                 ;; forward-char"]
    );
}

#[test]
fn describe_key_reads_a_sequence_says_what_it_runs_and_runs_nothing() {
    let (mut dispatcher, mut editor) = on_notes();
    send(&mut dispatcher, &mut editor, "C-h k");
    assert_eq!(echo(&editor), "Describe the following key: ");
    send(&mut dispatcher, &mut editor, "C-x");
    assert_eq!(echo(&editor), "C-x-");
    send(&mut dispatcher, &mut editor, "C-f");
    assert!(editor.prompt().is_none(), "find-file did not run");
    let (name, text) = shown(&editor);
    assert_eq!(name, "*Help*");
    assert!(
        text.starts_with("C-x C-f runs the command find-file.\n\n"),
        "{text}"
    );
    // The next key runs as usual.
    send(&mut dispatcher, &mut editor, "a");
    assert_eq!(shown(&editor).1, text, "*Help* is internal and read-only");
}

#[test]
fn describe_key_says_an_unbound_sequence_a_quit_and_a_shift_translation() {
    let (mut dispatcher, mut editor) = on_notes();
    send(&mut dispatcher, &mut editor, "C-h k C-x C-z");
    assert_eq!(echo(&editor), "C-x C-z is undefined");
    send(&mut dispatcher, &mut editor, "C-h k C-x C-g");
    assert_eq!(echo(&editor), "Quit");
    send(&mut dispatcher, &mut editor, "C-h k S-<right>");
    let (_, text) = shown(&editor);
    assert!(
        text.starts_with("S-<right> runs the command forward-char, bound to the key without Shift"),
        "{text}"
    );
    assert_eq!(shown(&editor).0, "*Help*");
    // Nothing ran: the notes' cursor is where it was.
    send(&mut dispatcher, &mut editor, "C-x b RET");
    assert_eq!(editor.window().selections.primary().cursor, 0);
}

#[test]
fn describe_key_in_a_terminal_says_what_goes_to_the_program_and_sends_nothing() {
    let (mut dispatcher, mut editor) = in_terminal();
    let terminal = editor.window().buffer();
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(notes);
    send(&mut dispatcher, &mut editor, "C-h k");
    editor.show(terminal);
    send(&mut dispatcher, &mut editor, "C-a");
    assert_eq!(echo(&editor), "C-a is sent to the terminal's program");
    send(&mut dispatcher, &mut editor, "C-x o");
    editor.show(notes);
    send(&mut dispatcher, &mut editor, "C-h k");
    editor.show(terminal);
    // `C-x` stays oso's in a terminal, so it is described through the keymaps.
    send(&mut dispatcher, &mut editor, "C-x 2");
    assert!(
        shown(&editor)
            .1
            .starts_with("C-x 2 runs the command split-window-below.")
    );
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
}

#[test]
fn keys_typed_in_a_prompt_over_a_buffer_that_may_hold_secrets_are_hidden() {
    let path = PathBuf::from("/test/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(&[path]));
    secret.finish_empty();
    let mut editor = Editor::with_buffer(secret);
    let mut dispatcher = Dispatcher::new();
    send(&mut dispatcher, &mut editor, "M-x");
    assert!(editor.prompt().is_some_and(|prompt| prompt.around_secret()));
    send(&mut dispatcher, &mut editor, "a b C-g");
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(notes);
    let kept = lossage(&mut dispatcher, &mut editor);
    assert_eq!(kept, ["<hidden>"]);
}

#[test]
fn a_keyboard_macro_s_replay_keeps_no_keys() {
    let (mut dispatcher, mut editor) = on_notes();
    send(&mut dispatcher, &mut editor, "C-x ( C-f C-x )");
    send(&mut dispatcher, &mut editor, "C-u 3 C-x e");
    while editor.macro_replaying() {
        dispatcher.replay(
            &mut editor,
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
    }
    let kept = lossage(&mut dispatcher, &mut editor);
    let forward = kept
        .iter()
        .filter(|line| line.contains("forward-char"))
        .count();
    assert_eq!(forward, 1, "{kept:?}");
}

#[test]
fn a_key_held_while_the_clipboard_is_awaited_is_kept_as_held_then_as_it_runs() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-y");
    assert!(dispatcher.waits_for_clipboard());
    send(&mut dispatcher, &mut editor, "p w");
    dispatcher.clipboard_answered(&mut editor, &FromClipboard::Nothing);
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("p w"));
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(notes);
    let kept = lossage(&mut dispatcher, &mut editor);
    assert_eq!(
        kept[1..],
        // A run of held keys is one line too.
        ["<held>", "<hidden>            ;; sent to the terminal"],
        "{kept:?}"
    );
}

#[test]
fn describe_key_reads_nothing_where_secrets_may_be() {
    let path = PathBuf::from("/test/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(&[path]));
    secret.finish_empty();
    let mut editor = Editor::with_buffer(secret);
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    let secret = editor.window().buffer();
    let mut dispatcher = Dispatcher::new();
    send(&mut dispatcher, &mut editor, "C-h k");
    assert_eq!(echo(&editor), "No key is described where secrets may be");
    assert!(!dispatcher.describing());
    // Begun elsewhere, it reads no key once the window shows such a buffer.
    editor.show(notes);
    send(&mut dispatcher, &mut editor, "C-h k");
    editor.show(secret);
    send(&mut dispatcher, &mut editor, "p");
    assert_eq!(echo(&editor), "No key is described where secrets may be");
    assert!(!dispatcher.describing());
    assert_ne!(shown(&editor).0, "*Help*");
}

#[test]
fn the_input_method_stays_off_while_describe_key_reads() {
    let (mut dispatcher, mut editor) = on_notes();
    assert!(!dispatcher.reads_keys() && dispatcher.types_text(&editor));
    send(&mut dispatcher, &mut editor, "C-h k");
    assert!(dispatcher.reads_keys() && !dispatcher.types_text(&editor));
}

#[test]
fn keys_at_a_passphrase_prompt_are_hidden() {
    let (mut dispatcher, mut editor) = on_notes();
    editor
        .open_prompt("Passphrase: ", Kind::Passphrase)
        .unwrap();
    send(&mut dispatcher, &mut editor, "s e c RET");
    assert_eq!(lossage(&mut dispatcher, &mut editor), ["<hidden>"]);
}

#[test]
fn every_key_to_a_terminal_reading_a_password_is_hidden() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.window().buffer();
    editor.set_echo_off(id, true);
    send(&mut dispatcher, &mut editor, "p w RET C-a");
    editor.set_echo_off(id, false);
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(notes);
    assert_eq!(lossage(&mut dispatcher, &mut editor), ["<hidden>"]);
}

#[test]
fn the_agent_channel_reads_nothing_of_the_lossage() {
    let (mut dispatcher, mut editor) = on_notes();
    send(&mut dispatcher, &mut editor, "a C-h l");
    assert_eq!(shown(&editor).0, "*Lossage*");
    let state = crate::channel::state::state(&editor, None, 0, &|_| None);
    assert!(state["text"].is_null(), "{state}");
}

#[test]
fn describe_key_hides_a_character_for_a_terminal_s_program() {
    let (mut dispatcher, mut editor) = in_terminal();
    let terminal = editor.window().buffer();
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(notes);
    send(&mut dispatcher, &mut editor, "C-h k");
    editor.show(terminal);
    send(&mut dispatcher, &mut editor, "p");
    assert_eq!(echo(&editor), "<char> is sent to the terminal's program");
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
}

#[test]
fn text_ends_the_read_and_a_restart_carries_none_of_it() {
    let (mut dispatcher, mut editor) = on_notes();
    send(&mut dispatcher, &mut editor, "C-h k C-x");
    assert!(dispatcher.describing());
    assert!(
        dispatcher.pending_to_carry().is_empty(),
        "nothing for a handoff"
    );
    assert_eq!(dispatcher.pending().len(), 1, "C-x is read");
    dispatcher.insert_text(&mut editor, "é");
    assert!(!dispatcher.describing());
    assert_eq!(shown(&editor).1, "éabc\n");
}

#[test]
fn a_key_with_no_emacs_name_is_hidden_where_secrets_may_be() {
    let path = PathBuf::from("/test/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(&[path]));
    secret.finish_empty();
    let mut editor = Editor::with_buffer(secret);
    let notes = editor.add_buffer(Buffer::from_text("notes", ""));
    let mut dispatcher = Dispatcher::new();
    dispatcher.note_unnamed_key(&editor);
    editor.show(notes);
    assert_eq!(lossage(&mut dispatcher, &mut editor), ["<hidden>"]);
}
