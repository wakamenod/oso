//! What the dispatcher publishes for the emergency save, and when (ADR 0062).
//!
//! Must not depend on: `app` or winit.

use std::path::PathBuf;

use oso_core::buffer::{Baseline, Buffer, Replacement};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;

use super::Dispatcher;
use crate::commands;
use crate::emergency::Emergency;
use crate::emergency::tests::{in_command, published_saving, published_text};

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

/// Starts a save, edits on, then panics.
fn save_then_panic(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.start_save().unwrap();
    let id = ctx.editor.window().buffer();
    let edit = Replacement {
        range: 0..0,
        text: "after the save ",
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
            reach: oso_core::command::Reach::Look,
            run: edit_then_panic,
            state: None,
        })
        .unwrap();
    registry
        .register(Command {
            name: "save-then-panic",
            doc: "Start a save, then panic.",
            args: ArgSpec::None,
            reach: oso_core::command::Reach::Look,
            run: save_then_panic,
            state: None,
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

#[test]
fn what_a_panicking_command_left_stays_held_back_through_a_report_until_a_command_ends() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..0,
        text: "good",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(published_text(&emergency, id).as_deref(), Some("good"));

    send(&mut dispatcher, &mut editor, "C-c p");
    // A save reporting, or a load, for this or any other buffer.
    dispatcher.publish_for_emergency(&editor);
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("good"),
        "what the panicking command left is not taken in"
    );

    // Panicking again changes nothing, and the next command that ends takes it all in.
    send(&mut dispatcher, &mut editor, "C-c p");
    dispatcher.publish_for_emergency(&editor);
    assert_eq!(published_text(&emergency, id).as_deref(), Some("good"));
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("half done half done good")
    );

    // A buffer with nothing published yet gets nothing from a panicking command.
    let other = editor.add_buffer(Buffer::from_text("other", ""));
    editor.show(other);
    send(&mut dispatcher, &mut editor, "C-c p");
    dispatcher.publish_for_emergency(&editor);
    assert_eq!(published_saving(&emergency, other), None);
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(
        published_text(&emergency, other).as_deref(),
        Some("half done ")
    );
}

#[test]
fn a_save_a_panicking_command_started_counts_with_the_text_it_is_writing() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = editor_with_a_file("text");
    let id = editor.window().buffer();

    let (_, result) = dispatcher.run_command(&mut editor, "save-then-panic", None);
    assert!(result.is_err());
    assert_eq!(published_saving(&emergency, id), Some(true));
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("text\n"),
        "the text on its way to the disk, with the final newline the save added, and no more"
    );
    assert_eq!(
        editor.current().1.text().to_string(),
        "after the save text\n"
    );

    // The save fails: the facts stay true, and the text stays.
    editor.save_failed(id);
    dispatcher.publish_for_emergency(&editor);
    assert_eq!(published_saving(&emergency, id), Some(false));
    assert_eq!(published_text(&emergency, id).as_deref(), Some("text\n"));
}

/// A file buffer with `text` typed into it, judged ordinary, in an editor showing it.
fn editor_with_a_file(text: &str) -> Editor {
    let mut buffer = Buffer::visiting(PathBuf::from("/p/a.txt"));
    buffer.finish_empty();
    buffer.judge_secret(Some(&[]));
    buffer.set_baseline(Baseline {
        location: PathBuf::from("/p/a.txt"),
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    let id = editor.window().buffer();
    let edit = Replacement { range: 0..0, text };
    editor.apply_edits(id, &[edit]).unwrap();
    editor
}

#[test]
fn a_save_already_running_lets_no_text_through_a_panicking_command() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = editor_with_a_file("good\n");
    let id = editor.window().buffer();
    let (_, result) = dispatcher.run_command(&mut editor, "save-buffer", None);
    assert_eq!(result, Ok(()));
    // As the app takes the job, right after that command.
    drop(editor.take_save_jobs());
    assert_eq!(published_saving(&emergency, id), Some(true));

    send(&mut dispatcher, &mut editor, "C-c p");
    assert_eq!(editor.current().1.text().to_string(), "half done good\n");
    assert_eq!(published_saving(&emergency, id), Some(true));
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("good\n"),
        "the save is writing what it was, and not what the command left"
    );
}

#[test]
fn a_save_an_earlier_key_in_the_batch_started_takes_nothing_back() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = editor_with_a_file("good\n");
    let id = editor.window().buffer();
    let (_, result) = dispatcher.run_command(&mut editor, "save-buffer", None);
    assert_eq!(result, Ok(()));
    // The queue is not taken between keys of one `send-keys`.
    let edit = Replacement {
        range: 0..0,
        text: "better ",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("better good\n")
    );

    send(&mut dispatcher, &mut editor, "C-c p");
    assert_eq!(
        published_text(&emergency, id).as_deref(),
        Some("better good\n"),
        "neither what the panic left nor the older text of the save"
    );
}

#[test]
fn a_secret_buffers_text_stays_out_even_if_a_panicking_command_started_its_save() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = editor_with_a_file("secret");
    let id = editor.window().buffer();
    let secret = [PathBuf::from("/h/.ssh/id_rsa")];
    editor.buffer_mut(id).unwrap().judge_secret(Some(&secret));
    let (_, result) = dispatcher.run_command(&mut editor, "save-then-panic", None);
    assert!(result.is_err());
    assert_eq!(published_saving(&emergency, id), Some(true), "known");
    assert_eq!(published_text(&emergency, id), None);
}

#[test]
fn a_verdict_that_forbids_a_copy_drops_the_text_even_while_held_back() {
    let emergency = Emergency::new(None);
    let mut dispatcher = dispatcher(&emergency);
    let mut editor = editor_with_a_file("text");
    let id = editor.window().buffer();
    send(&mut dispatcher, &mut editor, "C-f");
    assert_eq!(published_text(&emergency, id).as_deref(), Some("text"));

    send(&mut dispatcher, &mut editor, "C-c p");
    let secret = [PathBuf::from("/h/.ssh/id_rsa")];
    editor.buffer_mut(id).unwrap().judge_secret(Some(&secret));
    dispatcher.publish_for_emergency(&editor);
    assert_eq!(published_text(&emergency, id), None);
    assert_eq!(published_saving(&emergency, id), Some(false), "still known");
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
