//! A command that reads a directory and a name only with a prefix argument
//! (`ArgSpec::DirectoryAndTextOnPrefix`, ADR 0121): run at once without one, and with `C-u`
//! asked a directory, then a name, and run with both.
//!
//! Must not depend on: `app` or winit; these drive a dispatcher with a command of their own.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::Editor;

use super::Dispatcher;
use super::motion_tests::send;
use super::project_tests::list_directories;
use crate::commands::kept_messages;

/// Says what it was given.
fn say(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let read = ctx.directory_and_text();
    ctx.editor
        .message(format!("ran {read:?} {:?}", args.prefix()));
    Ok(())
}

fn dispatcher() -> Dispatcher {
    let mut registry = crate::commands::registry();
    let id = registry
        .register(Command {
            name: "say",
            doc: "Say what it was given.",
            args: ArgSpec::DirectoryAndTextOnPrefix {
                directory: "In directory:",
                directory_history: "say-directory",
                prompt: "Name:",
                history: "say",
            },
            reach: Reach::Person,
            run: say,
            state: None,
        })
        .unwrap();
    let mut global = crate::bindings::global_keymap(&registry);
    let keys: oso_core::key::KeySequence = "C-c s".parse().unwrap();
    global.bind(&keys.0, id).unwrap();
    Dispatcher::with(registry, global)
}

fn last(editor: &Editor) -> String {
    kept_messages(editor).pop().unwrap_or_default()
}

fn editor() -> Editor {
    let mut editor = Editor::default();
    editor.set_start_directory(std::path::PathBuf::from("/p/oso"));
    editor
}

#[test]
fn without_a_prefix_it_runs_at_once_with_nothing_read() {
    let (mut d, mut editor) = (dispatcher(), editor());
    send(&mut d, &mut editor, "C-c s");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran None None");
}

#[test]
fn with_c_u_it_reads_a_directory_then_a_name() {
    let (mut d, mut editor) = (dispatcher(), editor());
    send(&mut d, &mut editor, "C-u C-c s");
    let prompt = editor.prompt().expect("the directory's prompt");
    assert!(
        prompt.label().starts_with("In directory:"),
        "{}",
        prompt.label()
    );
    list_directories(&mut editor);
    send(&mut d, &mut editor, "RET");
    let prompt = editor.prompt().expect("the name's prompt");
    assert!(prompt.label().starts_with("Name:"), "{}", prompt.label());
    send(&mut d, &mut editor, "a p i RET");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some((\"/p/oso/\", \"api\")) Some(4)");
}

#[test]
fn quitting_either_prompt_runs_nothing() {
    let (mut d, mut editor) = (dispatcher(), editor());
    send(&mut d, &mut editor, "C-u C-c s C-g");
    assert!(editor.prompt().is_none());
    send(&mut d, &mut editor, "C-u C-c s");
    list_directories(&mut editor);
    send(&mut d, &mut editor, "RET C-g");
    assert!(editor.prompt().is_none());
    assert!(!last(&editor).starts_with("ran"), "{}", last(&editor));
}
