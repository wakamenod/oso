//! Scenario tests of what an input method gives the dispatcher: commits, each an
//! undo step of its own, the full-width space, and when keys are read as keys, which
//! turns the IME off (ADR 0069).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::buffer::Buffer;
use oso_core::editor::{Editor, Status};
use oso_core::key::KeySequence;
use oso_core::selection::{Selection, SelectionSet};
use oso_core::window::ViewSize;

use super::Dispatcher;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_bytes("test", text.as_bytes()));
    editor.set_view_size(ViewSize::new(10, 40));
    editor
}

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0);
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(&mut status);
    status.echo
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

#[test]
fn each_commit_is_an_undo_step_even_of_one_character() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), editor(""));
    send(&mut dispatcher, &mut editor, "a b");
    dispatcher.commit_text(&mut editor, "、");
    dispatcher.commit_text(&mut editor, "日本");
    send(&mut dispatcher, &mut editor, "c d");
    assert_eq!(text(&editor), "ab、日本cd");
    for left in ["ab、日本", "ab、", "ab", ""] {
        send(&mut dispatcher, &mut editor, "C-z");
        assert_eq!(text(&editor), left);
    }
}

#[test]
fn a_commit_goes_to_every_cursor() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), editor("ab\ncd\n"));
    let selections = vec![Selection::at(1), Selection::at(4)];
    editor.current_mut().0.selections = SelectionSet::from_parts(selections, 0).unwrap();
    let effects = dispatcher.commit_text(&mut editor, "かな");
    assert!(effects.redraw);
    assert_eq!(text(&editor), "aかなb\ncかなd\n");
}

#[test]
fn a_commit_after_a_pending_prefix_is_undefined_and_says_no_text() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), editor(""));
    send(&mut dispatcher, &mut editor, "C-x");
    dispatcher.commit_text(&mut editor, "秘密");
    assert_eq!(text(&editor), "");
    assert_eq!(echo(&editor), "C-x is undefined");
}

#[test]
fn keys_are_read_as_keys_after_a_prefix_key_or_argument() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), editor(""));
    assert!(!dispatcher.reads_keys());
    for (keys, reads) in [
        ("C-x", true),
        ("C-g", false),
        ("C-u", true),
        ("5", true),
        ("-", false),
    ] {
        send(&mut dispatcher, &mut editor, keys);
        assert_eq!(dispatcher.reads_keys(), reads, "after {keys}");
    }
    send(&mut dispatcher, &mut editor, "M-5");
    assert!(dispatcher.reads_keys(), "M-5 reads more digits");
    send(&mut dispatcher, &mut editor, "a");
    assert!(!dispatcher.reads_keys());
    assert_eq!(text(&editor), "-----aaaaa", "C-u 5 - typed five");
}

#[test]
fn option_space_inserts_a_full_width_space_that_undoes_with_typing() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), editor(""));
    send(&mut dispatcher, &mut editor, "a s-SPC b SPC");
    assert_eq!(text(&editor), "a\u{3000}b ");
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(text(&editor), "", "one step, as typed characters are");
    send(&mut dispatcher, &mut editor, "C-u 3 s-SPC");
    assert_eq!(text(&editor), "\u{3000}".repeat(3));
}
