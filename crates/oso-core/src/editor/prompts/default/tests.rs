//! Tests of a prompt's default (ADR 0138, step 14): `M-n` gives it from what was typed, `M-p`
//! gives that back, the history walks as before, and a passphrase takes none.
//!
//! Must not depend on: `command` or `keymap`.

use crate::buffer::{Buffer, Replacement};
use crate::editor::Editor;
use crate::minibuffer::{Kind, PromptError};

fn typed(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let edit = Replacement { range: 0..0, text };
    editor.apply_edits(id, &[edit]).unwrap();
}

fn shown(editor: &Editor) -> String {
    let id = editor.prompt().unwrap().buffer();
    editor.buffer(id).unwrap().text().to_string()
}

fn with_history(editor: &mut Editor, answers: &[&str]) {
    for answer in answers {
        let kind = Kind::Text {
            history: Some("lsp-rename"),
        };
        editor.open_prompt("Rename to: ", kind).unwrap();
        typed(editor, answer);
        editor.submit_prompt().unwrap();
    }
}

#[test]
fn m_n_gives_the_default_and_m_p_what_was_typed_back() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "let value = 1;\n"));
    with_history(&mut editor, &["older"]);
    let kind = Kind::Text {
        history: Some("lsp-rename"),
    };
    let prompt = editor.open_prompt("Rename ‘value’ to: ", kind).unwrap();
    editor.offer_prompt_default(prompt, "value");
    typed(&mut editor, "va");
    editor.walk_history(false).unwrap();
    assert_eq!(shown(&editor), "value");
    assert_eq!(
        editor.walk_history(false),
        Err(PromptError::Invalid),
        "past the default"
    );
    editor.walk_history(true).unwrap();
    assert_eq!(shown(&editor), "va", "what was typed comes back");
    editor.walk_history(true).unwrap();
    assert_eq!(shown(&editor), "older", "the history is walked as before");
    editor.walk_history(false).unwrap();
    assert_eq!(shown(&editor), "va");
    editor.walk_history(false).unwrap();
    assert_eq!(shown(&editor), "value");
}

#[test]
fn a_default_is_no_answer_by_itself_and_works_with_no_history() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "let value = 1;\n"));
    let prompt = editor
        .open_prompt("Say: ", Kind::Text { history: None })
        .unwrap();
    editor.offer_prompt_default(prompt, "hello");
    assert_eq!(shown(&editor), "", "the input stays empty");
    editor.walk_history(false).unwrap();
    assert_eq!(shown(&editor), "hello");
}

#[test]
fn a_passphrase_or_a_prompt_gone_takes_no_default() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "let value = 1;\n"));
    let prompt = editor
        .open_prompt("Passphrase: ", Kind::Passphrase)
        .unwrap();
    editor.offer_prompt_default(prompt, "secret");
    assert!(editor.walk_history(false).is_err());
    assert_eq!(shown(&editor), "");
    editor.submit_prompt().unwrap();
    editor.offer_prompt_default(prompt, "late");
    assert!(editor.prompt().is_none());
}
