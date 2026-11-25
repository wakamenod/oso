//! The keys of a native session's composer over the past prompts (ADR 0165, "The composer's
//! history, as amended"): `C-c C-r` lists them only over a composer that is not secret; and no
//! prompt's answer that a secret was yanked into goes into a composer.
//!
//! Must not depend on: `app` or winit; these drive the default dispatcher.

use std::sync::Arc;

use oso_core::buffer::{Buffer, Mode};
use oso_core::command::{Args, Invocation};
use oso_core::editor::{Editor, NativeAction};
use oso_core::kill::FromClipboard;

use super::Dispatcher;
use super::composer_tests::actions;
use super::motion_tests::send;

/// A dispatcher and an editor showing a composer, with past prompts to list.
fn with_past() -> (Dispatcher, Editor) {
    let mut editor = Editor::default();
    let composer = editor.add_buffer(Buffer::from_text("*compose: s*", ""));
    (editor.buffer_mut(composer).unwrap()).set_mode(Mode::Composer);
    editor.show(composer);
    editor.set_past_prompts(["newest one", "an older\none"].map(Arc::from).into());
    (Dispatcher::new(), editor)
}

#[test]
fn c_c_c_r_lists_past_prompts_and_ret_asks_for_the_one_chosen() {
    let (mut d, mut editor) = with_past();
    send(&mut d, &mut editor, "C-c C-r o l d e r RET");
    assert!(editor.prompt().is_none());
    assert_eq!(actions(&mut editor), [NativeAction::PastPrompt]);
    assert_eq!(editor.take_past_prompt().as_deref(), Some("an older\none"));
    // What matches nothing is not taken.
    send(&mut d, &mut editor, "C-c C-r z z RET");
    assert!(editor.prompt().is_some());
    send(&mut d, &mut editor, "C-g");
    assert!(actions(&mut editor).is_empty());
    assert_eq!(editor.take_past_prompt(), None);
    // And none of it is in a history.
    send(&mut d, &mut editor, "C-c C-r M-p");
    let answer = editor.prompt().unwrap().buffer();
    assert!(editor.buffer(answer).unwrap().text().is_empty());
}

#[test]
fn past_prompts_are_listed_only_over_a_composer_that_is_not_secret() {
    let (mut d, mut editor) = with_past();
    let secret = editor.add_buffer(Buffer::visiting("/tmp/.env".into()));
    (editor.buffer_mut(secret).unwrap()).set_mode(Mode::Composer);
    editor.show(secret);
    send(&mut d, &mut editor, "C-c C-r");
    assert!(editor.prompt().is_none());
    let plain = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(plain);
    let name = "native-composer-past-prompt";
    let id = d.registry.lookup(name).unwrap();
    let (args, invoked) = (Args::NONE, Invocation::default());
    let _ = d.run_or_read(&mut editor, id, args, invoked);
    assert!(editor.prompt().is_none());
    assert!(actions(&mut editor).is_empty());
}

#[test]
fn a_model_that_a_secret_was_yanked_into_is_not_put_in_the_composer() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let buffer = editor.add_buffer(Buffer::from_text("*compose: s*", "hi"));
    (editor.buffer_mut(buffer).unwrap()).set_mode(Mode::Composer);
    editor.show(buffer);
    let id = d.registry.lookup("native-composer-model").unwrap();
    editor.kill_new("hunter2", true);
    let (args, invoked) = (Args::NONE, Invocation::default());
    let _ = d.run_or_read(&mut editor, id, args, invoked);
    assert!(editor.prompt().is_some());
    send(&mut d, &mut editor, "C-y");
    assert!(d.take_clipboard_request().is_some());
    d.clipboard_answered(&mut editor, &FromClipboard::Own);
    send(&mut d, &mut editor, "RET");
    assert!(editor.prompt().is_none());
    assert_eq!(editor.current().1.text().to_string(), "hi");
}
