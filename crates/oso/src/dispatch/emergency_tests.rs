//! What the dispatcher publishes for the emergency save, and when (ADR 0062).
//!
//! Must not depend on: `app` or winit.

use oso_core::buffer::{Buffer, Replacement};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;

use super::Dispatcher;
use crate::commands;
use crate::emergency::Emergency;
use crate::emergency::tests::{in_command, published_text};

/// Edits the buffer, then panics.
fn edit_then_panic(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let id = ctx.editor.window().buffer();
    let edit = Replacement {
        range: 0..0,
        text: "half done ",
    };
    ctx.editor.apply_edits(id, &[edit])?;
    panic!("deliberate panic in a test command");
}

fn dispatcher(emergency: &Emergency) -> Dispatcher {
    let mut registry = commands::registry();
    registry
        .register(Command {
            name: "edit-then-panic",
            doc: "Edit, then panic.",
            args: ArgSpec::None,
            run: edit_then_panic,
        })
        .unwrap();
    let global = Keymap::from_table(
        &registry,
        &[("C-c p", "edit-then-panic"), ("C-f", "forward-char")],
    )
    .unwrap();
    let mut dispatcher = Dispatcher::with(registry, global);
    dispatcher.set_publisher(emergency.publisher());
    dispatcher
}

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0);
}

#[test]
fn each_command_publishes_the_text_at_risk_and_a_panicking_one_does_not() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = Editor::with_buffer(Buffer::from_text("t", "abc"));
    let id = editor.window().buffer();

    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(published_text(&emergency, id), None, "nothing is at risk");

    let edit = Replacement {
        range: 0..0,
        text: "x",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    assert_eq!(published_text(&emergency, id), None, "only after a command");
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(published_text(&emergency, id).as_deref(), Some("xabc"));

    send(&mut dispatcher, &mut editor, "C-c p");
    assert_eq!(editor.current().1.text().to_string(), "half done xabc");
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("xabc"),
        "what a panicking command left is not published"
    );

    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("half done xabc"),
        "the next command takes in what the person now sees"
    );

    let content = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(content);
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(
        published_text(&emergency, id),
        None,
        "saved: nothing at risk"
    );
}

// That the mark is there while a command runs shows in the end-to-end flow: a command's
// panic writes nothing, which it would if the hook saw no mark.
#[test]
fn the_boundary_is_not_marked_after_a_command_even_after_a_panic() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = Editor::with_buffer(Buffer::from_text("t", "abc"));
    let (_, result) = dispatcher.run_command(&mut editor, "edit-then-panic", None);
    assert!(result.is_err());
    assert!(!in_command(&emergency), "the panic took the mark with it");
    send(&mut dispatcher, &mut editor, "C-f");
    assert!(!in_command(&emergency));
}
