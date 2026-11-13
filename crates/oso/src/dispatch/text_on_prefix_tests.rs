//! A command whose text is read only with a prefix argument (`ArgSpec::TextOnPrefix`): run at
//! once without one, asked with `C-u`, and replayed by a keyboard macro as typed.
//!
//! Must not depend on: `app` or winit; these drive a dispatcher with a command of their own.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::Editor;

use super::Dispatcher;
use super::motion_tests::send;
use crate::commands::kept_messages;

/// Says what it was given.
fn say(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let answer = ctx.answer().map(str::to_owned);
    ctx.editor
        .message(format!("ran {answer:?} {:?}", args.prefix()));
    Ok(())
}

fn dispatcher() -> Dispatcher {
    let mut registry = crate::commands::registry();
    let id = registry
        .register(Command {
            name: "say",
            doc: "Say what it was given.",
            args: ArgSpec::TextOnPrefix {
                prompt: "Base: ",
                history: "say",
            },
            reach: Reach::Look,
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

#[test]
fn without_a_prefix_it_runs_at_once_with_no_answer() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-c s");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran None None");
}

#[test]
fn with_c_u_it_asks_and_runs_with_the_answer_and_the_prefix() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-u C-c s");
    assert!(editor.prompt().is_some());
    send(&mut d, &mut editor, "m a i n RET");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some(\"main\") Some(4)");
}

#[test]
fn a_keyboard_macro_replays_the_question_and_its_answer() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-x ( C-u C-c s d e v RET C-x )");
    assert_eq!(last(&editor), "ran Some(\"dev\") Some(4)");
    editor.message("between");
    send(&mut d, &mut editor, "C-x e");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some(\"dev\") Some(4)");
}
