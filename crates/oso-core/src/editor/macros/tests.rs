//! Tests of keyboard macros on the editor (ADR 0113): recording, its bounds, replaying with a
//! count, what a replay changed undoing as one step, and secrets, questions and passphrases.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher records and feeds in the tip crate.

use super::super::test_buffers::secret_buffer;
use super::{MAX_MACRO_EVENTS, MAX_MACRO_TEXT, MacroError, MacroEvent};
use crate::buffer::{Buffer, Replacement};
use crate::editor::Editor;
use crate::minibuffer::Kind;

fn text_event(text: &str) -> MacroEvent {
    MacroEvent::Text {
        text: text.to_owned(),
        commit: false,
    }
}

fn editor() -> Editor {
    Editor::with_buffer(Buffer::from_text("notes.txt", "abc"))
}

#[test]
fn a_recording_becomes_the_last_macro_and_replays_count_times() {
    let mut editor = editor();
    assert_eq!(editor.macro_call(Some(1)), Err(MacroError::NoMacro));
    editor.macro_start().unwrap();
    assert_eq!(editor.macro_start(), Err(MacroError::AlreadyRecording));
    editor.macro_record([text_event("x"), text_event("y")]);
    editor.macro_end().unwrap();
    assert!(!editor.macro_recording());
    editor.macro_call(Some(2)).unwrap();
    let mut fed = Vec::new();
    while let Some(event) = editor.macro_next() {
        fed.push(event);
    }
    assert_eq!(
        fed,
        [
            text_event("x"),
            text_event("y"),
            text_event("x"),
            text_event("y")
        ]
    );
    assert!(!editor.macro_replaying());
}

#[test]
fn a_recording_past_its_bounds_is_dropped_and_the_last_macro_stays() {
    let mut editor = editor();
    editor.macro_start().unwrap();
    editor.macro_record([text_event("kept")]);
    editor.macro_end().unwrap();
    editor.macro_start().unwrap();
    editor.macro_record((0..=MAX_MACRO_EVENTS).map(|_| text_event("x")));
    assert!(!editor.macro_recording());
    editor.macro_call(Some(1)).unwrap();
    assert_eq!(editor.macro_next(), Some(text_event("kept")));
}

#[test]
fn until_stopped_it_goes_round_and_what_it_changed_undoes_at_once() {
    let mut editor = editor();
    editor.macro_start().unwrap();
    editor.macro_record([text_event("x")]);
    editor.macro_end().unwrap();
    editor.macro_call(None).unwrap();
    let id = editor.window().buffer();
    for _ in 0..5 {
        assert!(editor.macro_next().is_some());
        let end = editor.current().1.text().len();
        editor.begin_undo_group();
        editor
            .apply_edits(
                id,
                &[Replacement {
                    range: end..end,
                    text: "x",
                }],
            )
            .unwrap();
        editor.end_undo_group();
    }
    assert!(editor.macro_replaying(), "it goes on until stopped");
    editor.macro_stop(Some("stopped"));
    assert_eq!(editor.current().1.text().to_string(), "abcxxxxx");
    editor.undo().unwrap();
    assert_eq!(editor.current().1.text().to_string(), "abc");
}

#[test]
fn no_macro_command_runs_inside_a_replay_or_a_replay_inside_a_recording() {
    let mut editor = editor();
    editor.macro_start().unwrap();
    editor.macro_record([text_event("x")]);
    assert_eq!(
        editor.macro_call(Some(1)),
        Err(MacroError::AlreadyRecording)
    );
    editor.macro_end().unwrap();
    editor.macro_call(Some(1)).unwrap();
    assert_eq!(editor.macro_start(), Err(MacroError::Replaying));
    assert_eq!(format!("{:?}", text_event("secret")), "Text");
}

/// An editor with a last macro that types "x".
fn with_macro() -> Editor {
    let mut editor = editor();
    editor.macro_start().unwrap();
    editor.macro_record([text_event("x")]);
    editor.macro_end().unwrap();
    editor
}

#[test]
fn text_past_the_byte_cap_drops_the_recording_before_it_is_copied() {
    let mut editor = with_macro();
    editor.macro_start().unwrap();
    assert!(editor.macro_takes_text(MAX_MACRO_TEXT));
    assert!(!editor.macro_takes_text(MAX_MACRO_TEXT + 1));
    assert!(!editor.macro_recording());
    editor.macro_start().unwrap();
    editor.macro_record([text_event(&"a".repeat(MAX_MACRO_TEXT / 2))]);
    editor.macro_record([text_event(&"a".repeat(MAX_MACRO_TEXT / 2 + 1))]);
    assert!(!editor.macro_recording());
    editor.macro_call(Some(1)).unwrap();
    assert_eq!(editor.macro_next(), Some(text_event("x")), "the last stays");
}

#[test]
fn input_to_a_buffer_that_may_hold_secrets_drops_the_recording() {
    let mut editor = Editor::with_buffer(secret_buffer());
    editor.macro_start().unwrap();
    editor.macro_record([text_event("hunter2")]);
    assert!(!editor.macro_recording());
    assert_eq!(editor.macro_call(Some(1)), Err(MacroError::NoMacro));
}

#[test]
fn no_replay_starts_while_a_question_or_a_passphrase_is_asked() {
    let mut editor = with_macro();
    editor.ask("Exit?", "save-buffers-kill-terminal").unwrap();
    assert_eq!(editor.macro_call(Some(1)), Err(MacroError::Asking));
    editor.quit();
    editor
        .open_prompt("Passphrase: ", Kind::Passphrase)
        .unwrap();
    assert_eq!(editor.macro_call(Some(1)), Err(MacroError::Asking));
}

#[test]
fn a_passphrase_prompt_drops_the_recording_and_stops_a_replay_as_it_opens() {
    let mut editor = with_macro();
    editor.macro_start().unwrap();
    editor
        .open_prompt("Passphrase: ", Kind::Passphrase)
        .unwrap();
    assert!(!editor.macro_recording());
    editor.quit();
    editor.macro_call(None).unwrap();
    assert!(editor.macro_next().is_some());
    editor
        .open_prompt("Passphrase: ", Kind::Passphrase)
        .unwrap();
    assert!(!editor.macro_replaying());
}

#[test]
fn the_last_macro_is_forgotten_and_its_replay_stopped() {
    let mut editor = editor();
    editor.macro_start().unwrap();
    editor.macro_record([text_event("x"), text_event("y")]);
    editor.macro_end().unwrap();
    editor.macro_call(Some(1)).unwrap();
    assert!(editor.macro_replaying());
    editor.macro_forget_last("dropped");
    assert!(!editor.macro_replaying());
    assert_eq!(editor.macro_next(), None);
    assert_eq!(editor.macro_call(Some(1)), Err(MacroError::NoMacro));
    // Nothing to forget says nothing more.
    editor.macro_forget_last("again");
}
