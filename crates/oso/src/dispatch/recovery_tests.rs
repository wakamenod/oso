//! The keys of the emergency copies through the [`Dispatcher`] (ADR 0064): which keymap is on
//! when, with no files, as `emergency::offer`'s tests have those.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use oso_core::buffer::{Buffer, Mode};
use oso_core::command::Effects;
use oso_core::editor::{Editor, RecoveryAction};
use oso_core::key::KeySequence;

use super::Dispatcher;

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) -> Effects {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0)
}

fn on_a_file() -> (Dispatcher, Editor) {
    let editor = Editor::with_buffer(Buffer::from_text("notes.txt", "abc\n"));
    (Dispatcher::new(), editor)
}

#[test]
fn the_keys_answer_only_in_the_list_and_type_everywhere_else() {
    let (mut dispatcher, mut editor) = on_a_file();
    send(&mut dispatcher, &mut editor, "r d v");
    assert_eq!(editor.current().1.text().to_string(), "rdvabc\n");
    assert_eq!(editor.take_recovery_actions(), []);

    send(&mut dispatcher, &mut editor, "C-h r");
    assert_eq!(editor.current().1.mode(), Mode::CopyList);
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Toggled]);
    send(&mut dispatcher, &mut editor, "v");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::View(0)]);
    send(&mut dispatcher, &mut editor, "RET");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::View(0)]);
    send(&mut dispatcher, &mut editor, "=");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Compare(0)]);
    send(&mut dispatcher, &mut editor, "r");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Recover(0)]);
    send(&mut dispatcher, &mut editor, "g");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Refresh]);
}

#[test]
fn a_letter_with_no_answer_is_refused_there_and_motion_still_works() {
    let (mut dispatcher, mut editor) = on_a_file();
    send(&mut dispatcher, &mut editor, "C-h r x");
    assert_eq!(editor.current().1.text().to_string(), "");
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(editor.current().1.mode(), Mode::CopyList);
}

#[test]
fn d_needs_two_in_a_row_and_any_other_key_cancels_it() {
    let (mut dispatcher, mut editor) = on_a_file();
    send(&mut dispatcher, &mut editor, "C-h r");
    editor.take_recovery_actions();
    let marks = [
        RecoveryAction::ForgetMarks,
        RecoveryAction::MarkForDelete(0),
    ];
    send(&mut dispatcher, &mut editor, "d");
    assert_eq!(editor.take_recovery_actions(), marks);
    send(&mut dispatcher, &mut editor, "C-f d");
    assert_eq!(
        editor.take_recovery_actions(),
        marks,
        "another key came between"
    );
    send(&mut dispatcher, &mut editor, "d");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Delete(0)]);
    send(&mut dispatcher, &mut editor, "d");
    assert_eq!(
        editor.take_recovery_actions(),
        marks,
        "the third starts over"
    );
}

#[test]
fn q_goes_back_and_the_keys_type_again() {
    let (mut dispatcher, mut editor) = on_a_file();
    send(&mut dispatcher, &mut editor, "C-h r q r");
    assert_eq!(editor.current().1.mode(), Mode::Fundamental);
    assert_eq!(editor.current().1.text().to_string(), "rabc\n");
}

#[test]
fn a_buffer_showing_a_copy_takes_fewer_keys() {
    let (mut dispatcher, mut editor) = on_a_file();
    send(&mut dispatcher, &mut editor, "C-h r");
    let view = editor.add_internal_buffer("*copy of notes.txt*", Mode::CopyView);
    editor.show(view);
    editor.take_recovery_actions();
    send(&mut dispatcher, &mut editor, "r");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Recover(0)]);
    send(&mut dispatcher, &mut editor, "=");
    assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Compare(0)]);
    send(&mut dispatcher, &mut editor, "d d v g");
    assert_eq!(editor.take_recovery_actions(), [], "those are the list's");
    send(&mut dispatcher, &mut editor, "q");
    assert_eq!(editor.current().1.mode(), Mode::CopyList);
}

#[test]
fn messages_and_the_list_go_back_through_each_other() {
    let (mut dispatcher, mut editor) = on_a_file();
    send(&mut dispatcher, &mut editor, "C-h r C-h e");
    assert_eq!(editor.current().1.name(), "*Messages*");
    send(&mut dispatcher, &mut editor, "C-h e");
    assert_eq!(editor.current().1.mode(), Mode::CopyList);
    send(&mut dispatcher, &mut editor, "q");
    assert_eq!(editor.current().1.name(), "notes.txt");
}
