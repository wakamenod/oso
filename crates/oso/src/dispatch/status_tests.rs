//! What keys leave in the echo area, and in the log's way: failures, undefined keys and
//! what is being typed (ADR 0058, ADR 0063).
//!
//! Must not depend on: `app`, winit or the renderer; these are plain functions of the
//! editor state.

use std::path::PathBuf;

use oso_core::buffer::Buffer;
use oso_core::editor::{Editor, Status};
use oso_core::key::KeySequence;
use oso_core::load::decode_all;
use oso_core::window::ViewSize;

use super::Dispatcher;

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0);
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(&mut status);
    status.echo
}

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("test", text));
    editor.set_view_size(ViewSize::new(5, 40));
    editor
}

/// An editor showing a loaded file that may hold secrets.
fn on_a_secret(text: &str) -> Editor {
    let path = PathBuf::from("/tmp/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(text.as_bytes(), Default::default()));
    buffer.judge_secret(Some(&[path]));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(5, 40));
    editor
}

#[test]
fn a_failed_command_shows_until_the_next_key() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    send(&mut dispatcher, &mut editor, "C-b");
    assert_eq!(echo(&editor), "Beginning of buffer");
    assert_eq!(editor.messages().count(), 1);
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(echo(&editor), "");
    assert_eq!(editor.messages().count(), 1, "it stays in *Messages*");
}

#[test]
fn run_command_empties_the_echo_area_too() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    send(&mut dispatcher, &mut editor, "C-b");
    let (_, result) = dispatcher.run_command(&mut editor, "forward-char", None);
    assert_eq!(result, Ok(()));
    assert_eq!(echo(&editor), "");
}

#[test]
fn an_undefined_key_reaches_the_echo_area_and_nothing_else() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    send(&mut dispatcher, &mut editor, "C-x C-z");
    assert_eq!(echo(&editor), "C-x C-z is undefined");
    send(&mut dispatcher, &mut editor, "C-x Ж");
    assert_eq!(echo(&editor), "C-x Ж is undefined");
    send(&mut dispatcher, &mut editor, "<f35>");
    assert_eq!(echo(&editor), "<f35> is undefined");
    assert_eq!(editor.messages().count(), 0);
}

#[test]
fn text_after_a_prefix_is_not_quoted() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    send(&mut dispatcher, &mut editor, "C-x");
    dispatcher.insert_text(&mut editor, "typed");
    assert_eq!(echo(&editor), "C-x is undefined");
    assert_eq!(editor.messages().count(), 0);
}

#[test]
fn quitting_says_so_whichever_way() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    send(&mut dispatcher, &mut editor, "C-g");
    assert_eq!(echo(&editor), "Quit");
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(echo(&editor), "");
    send(&mut dispatcher, &mut editor, "C-x C-g");
    assert_eq!(echo(&editor), "Quit");
}

#[test]
fn a_pending_prefix_key_shows_until_it_resolves() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    send(&mut dispatcher, &mut editor, "C-x");
    assert_eq!(echo(&editor), "C-x-");
    send(&mut dispatcher, &mut editor, "C-g");
    assert_eq!(echo(&editor), "Quit");
}

#[test]
fn a_prefix_argument_shows_as_it_is_typed() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("abcdefghijklmnopqrstuvwxyz");
    for (keys, shown) in [
        ("C-u", "C-u-"),
        ("C-u", "C-u 16-"),
        ("3", "C-u 3-"),
        ("5", "C-u 35-"),
        ("C-x", "C-u 35 C-x-"),
    ] {
        send(&mut dispatcher, &mut editor, keys);
        assert_eq!(echo(&editor), shown, "after {keys}");
    }
    send(&mut dispatcher, &mut editor, "C-g");
    assert_eq!(echo(&editor), "Quit");

    send(&mut dispatcher, &mut editor, "M--");
    assert_eq!(echo(&editor), "C-u --");
    send(&mut dispatcher, &mut editor, "5");
    assert_eq!(echo(&editor), "C-u -5-");
    send(&mut dispatcher, &mut editor, "C-b");
    assert_eq!(echo(&editor), "", "the command used it");

    send(&mut dispatcher, &mut editor, "M-3 C-f");
    assert_eq!(echo(&editor), "");
}

#[test]
fn a_prefix_handed_over_shows_too() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    let keys: KeySequence = "C-x".parse().unwrap();
    dispatcher.restore_pending(&mut editor, &keys.0);
    assert_eq!(echo(&editor), "C-x-");
}

#[test]
fn a_failure_around_a_secret_stays_in_the_echo_area() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = on_a_secret("KEY=value\n");
    send(&mut dispatcher, &mut editor, "C-b");
    assert_eq!(echo(&editor), "Beginning of buffer", "for the person there");
    assert_eq!(editor.messages().count(), 0, "not in the log or *Messages*");
    send(&mut dispatcher, &mut editor, "M->");
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(echo(&editor), "End of buffer");
    assert_eq!(editor.messages().count(), 0);

    // Out of view, it no longer shows, whichever way the buffer went.
    let other = editor.add_buffer(Buffer::new("other"));
    editor.show(other);
    assert_eq!(echo(&editor), "");
    send(&mut dispatcher, &mut editor, "C-h e");
    assert_eq!(
        editor.current().1.text().len(),
        0,
        "*Messages* holds nothing"
    );
}

#[test]
fn a_message_a_command_leaves_around_a_secret_stays_out_of_the_log_too() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = on_a_secret("KEY=value\n");
    // Saving an unmodified buffer calls `Editor::message` itself, and returns no error.
    send(&mut dispatcher, &mut editor, "C-x C-s");
    let _ = editor.take_save_jobs();
    assert_eq!(editor.messages().count(), 0);
}

#[test]
fn an_undefined_key_around_a_secret_goes_with_it() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = on_a_secret("KEY=value\n");
    send(&mut dispatcher, &mut editor, "C-x Ж");
    assert_eq!(echo(&editor), "C-x Ж is undefined");
    let other = editor.add_buffer(Buffer::new("other"));
    editor.show(other);
    assert_eq!(echo(&editor), "");
}

#[test]
fn a_key_in_messages_cannot_change_them() {
    let mut dispatcher = Dispatcher::new();
    let mut editor = editor("ab");
    send(&mut dispatcher, &mut editor, "C-b C-h e");
    let shown = editor.current().1.text().to_string();
    assert_eq!(shown, "Beginning of buffer\n");
    send(&mut dispatcher, &mut editor, "a");
    assert_eq!(echo(&editor), "Buffer is read-only");
    assert!(!editor.current().1.is_modified());
    assert_eq!(
        editor.current().1.text().to_string(),
        "Beginning of buffer\nBuffer is read-only\n"
    );
    send(&mut dispatcher, &mut editor, "C-h e");
    assert_eq!(editor.current().1.name(), "test");
}
