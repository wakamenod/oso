//! Scenario tests of `M-y` with its keys, through the [`Dispatcher`] (ADR 0037, ADR 0091):
//! the kill ring listed after the clipboard is read, a kill picked with `RET`, and `M-y`
//! right after a yank in a prompt and in isearch.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with
//! no window. The clipboard's answer is given here, as the host would.

use oso_core::editor::Editor;
use oso_core::kill::{Fingerprint, FromClipboard};
use oso_core::rope::Rope;

use super::Dispatcher;
use super::motion_tests::{at, send, text};
use crate::commands::kept_messages;

/// `keys`, which read the clipboard, and its answer.
fn answered(d: &mut Dispatcher, editor: &mut Editor, keys: &str, answer: &FromClipboard) {
    send(d, editor, keys);
    assert!(
        d.take_clipboard_request().is_some(),
        "{keys} reads the clipboard"
    );
    d.clipboard_answered(editor, answer);
}

fn from_app(text: &str) -> FromClipboard {
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
fn m_y_lists_the_ring_with_the_clipboard_first_and_yanks_the_one_picked() {
    let (mut d, mut editor) = at("one two\n", &[0]);
    send(&mut d, &mut editor, "M-d M-f");
    answered(&mut d, &mut editor, "M-y", &from_app("copied"));
    assert!(editor.kill_list_open());
    send(&mut d, &mut editor, "C-n RET");
    assert!(!editor.kill_list_open());
    assert_eq!(text(&editor), " twoone\n");
    assert_eq!(
        editor.kill_ring().get(0).unwrap().joined(),
        "one",
        "to the front"
    );
}

#[test]
fn m_y_right_after_a_yank_puts_the_one_picked_in_its_place() {
    // Two kills apart: `one`, then `two`.
    let (mut d, mut editor) = at("one two\n", &[0]);
    send(&mut d, &mut editor, "M-d C-f M-d");
    answered(&mut d, &mut editor, "C-y", &FromClipboard::Own);
    assert_eq!(text(&editor), " two\n");
    answered(&mut d, &mut editor, "M-y", &FromClipboard::Own);
    send(&mut d, &mut editor, "C-n RET");
    assert_eq!(
        text(&editor),
        " one\n",
        "the older kill in place of the newer"
    );
}

#[test]
fn m_y_with_nothing_to_list_says_so() {
    let (mut d, mut editor) = at("one\n", &[0]);
    answered(&mut d, &mut editor, "M-y", &FromClipboard::Nothing);
    assert!(!editor.kill_list_open());
    assert_eq!(
        kept_messages(&editor).pop().as_deref(),
        Some("Kill ring is empty")
    );
}

#[test]
fn m_y_in_a_prompt_and_in_isearch_goes_to_the_older_kill_after_a_yank() {
    let (mut d, mut editor) = at("one two\n", &[0]);
    send(&mut d, &mut editor, "M-d C-f M-d");
    send(&mut d, &mut editor, "M-x");
    answered(&mut d, &mut editor, "C-y", &FromClipboard::Own);
    send(&mut d, &mut editor, "M-y");
    let id = editor.prompt().unwrap().buffer();
    assert_eq!(editor.buffer(id).unwrap().text().to_string(), "one");
    send(&mut d, &mut editor, "C-g C-s C-y M-y");
    assert_eq!(editor.isearch().unwrap().string(), "one");
    send(&mut d, &mut editor, "M-y");
    assert_eq!(
        editor.isearch().unwrap().string(),
        "two",
        "round to the newest"
    );
}
