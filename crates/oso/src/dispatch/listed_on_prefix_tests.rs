//! A command whose list is read only with a prefix argument (`ArgSpec::ListedOnPrefix`, ADR
//! 0122 step 12): run at once without one, and with `C-u` asked with completion over the list
//! the host gave, then run with the answer and the prefix. The agent channel runs it with none.
//!
//! Must not depend on: `app` or winit; these drive a dispatcher with a command of their own.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::complete::Candidate;
use oso_core::editor::Editor;

use super::Dispatcher;
use super::motion_tests::send;
use crate::commands::kept_messages;

const LIST: &str = "test-sessions";

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
            args: ArgSpec::ListedOnPrefix {
                list: LIST,
                prompt: "Session: ",
                history: "say",
            },
            reach: Reach::Change,
            run: say,
            state: None,
        })
        .unwrap();
    let mut global = crate::bindings::global_keymap(&registry);
    let keys: oso_core::key::KeySequence = "C-c s".parse().unwrap();
    global.bind(&keys.0, id).unwrap();
    Dispatcher::with(registry, global)
}

fn editor() -> Editor {
    let mut editor = Editor::default();
    let sessions = [
        Candidate::new("*claude: one*", "", ""),
        Candidate::new("*claude: two*", "", ""),
    ];
    editor.set_listed(LIST, sessions.into_iter().collect());
    editor
}

fn last(editor: &Editor) -> String {
    kept_messages(editor).pop().unwrap_or_default()
}

#[test]
fn without_a_prefix_it_runs_at_once_with_no_answer() {
    let (mut d, mut editor) = (dispatcher(), editor());
    send(&mut d, &mut editor, "C-c s");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran None None");
}

#[test]
fn with_c_u_it_completes_over_the_list_and_runs_with_the_answer() {
    let (mut d, mut editor) = (dispatcher(), editor());
    send(&mut d, &mut editor, "C-u C-c s");
    assert!(editor.prompt().is_some());
    assert_eq!(editor.listed_candidates(LIST).len(), 2);
    send(&mut d, &mut editor, "t w o RET");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some(\"*claude: two*\") Some(4)");
}

#[test]
fn the_agent_channel_runs_it_with_no_answer_and_gives_it_none() {
    use super::args::Given;
    let (mut d, mut editor) = (dispatcher(), editor());
    let (_, result) = d
        .run_for_agent(&mut editor, "say", Given::Nothing, Reach::Change)
        .unwrap();
    assert_eq!(result, Ok(()));
    assert_eq!(last(&editor), "ran None None");
    assert!(
        d.run_for_agent(
            &mut editor,
            "say",
            Given::Text("*claude: one*"),
            Reach::Change
        )
        .is_err()
    );
}
