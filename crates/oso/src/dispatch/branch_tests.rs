//! A command that reads a branch of the project (`ArgSpec::BranchAndBaseOnPrefix`, ADR 0121):
//! it waits for the host's list of branches, then reads a branch over them, and with `C-u` a
//! base after it; a wait is dropped when a prompt opened meanwhile, and an error is said.
//!
//! Must not depend on: `app` or winit; these drive a dispatcher with a command of their own.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::Editor;

use super::Dispatcher;
use super::motion_tests::send;
use crate::commands::kept_messages;

/// Says what it was given.
fn say(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let read = ctx.branch_and_base();
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
            args: ArgSpec::BranchAndBaseOnPrefix {
                prompt: "Branch:",
                base_prompt: "Base:",
                history: "say-branch",
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

fn branches() -> Result<Vec<String>, String> {
    Ok(vec!["main".into(), "feature/api".into()])
}

/// The labels of the prompts open, the outermost first.
fn labels(editor: &Editor) -> Vec<String> {
    let prompts = editor.prompts();
    prompts.iter().map(|p| p.label().to_owned()).collect()
}

#[test]
fn it_waits_for_the_branches_before_its_prompt_opens() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    assert!(!d.take_branches_request(), "nothing waits");
    send(&mut d, &mut editor, "C-c s");
    assert!(editor.prompt().is_none(), "no prompt until the list comes");
    assert!(d.take_branches_request());
    assert!(!d.take_branches_request(), "asked once");
    assert!(d.branches_answered(&mut editor, branches()).redraw);
    assert_eq!(labels(&editor), ["Branch:"]);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    let rows: Vec<&str> = candidates.shown(18).map(|(_, c)| c.text()).collect();
    assert_eq!(rows, ["main", "feature/api"], "in the host's order");
}

#[test]
fn without_a_prefix_a_branch_is_read_and_no_base() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-c s");
    d.take_branches_request();
    d.branches_answered(&mut editor, branches());
    send(&mut d, &mut editor, "f e a t RET");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some((\"feature/api\", None)) None");
}

#[test]
fn a_name_that_is_no_branch_is_taken_as_typed() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-c s");
    d.take_branches_request();
    d.branches_answered(&mut editor, branches());
    send(&mut d, &mut editor, "n e w x RET");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some((\"newx\", None)) None");
}

#[test]
fn with_c_u_a_base_is_read_after_the_branch() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-u C-c s");
    assert!(d.take_branches_request());
    d.branches_answered(&mut editor, branches());
    send(&mut d, &mut editor, "f i x RET");
    assert_eq!(labels(&editor), ["Base:"]);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.shown(18).count(), 2, "over the same branches");
    send(&mut d, &mut editor, "m a i n RET");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some((\"fix\", Some(\"main\"))) Some(4)");
}

#[test]
fn an_empty_base_stands_for_none() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-u C-c s");
    d.take_branches_request();
    d.branches_answered(&mut editor, Ok(Vec::new()));
    send(&mut d, &mut editor, "f i x RET RET");
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "ran Some((\"fix\", None)) Some(4)");
}

#[test]
fn a_prompt_opened_meanwhile_drops_the_wait() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-c s");
    d.take_branches_request();
    send(&mut d, &mut editor, "M-x");
    assert_eq!(editor.prompts().len(), 1);
    d.branches_answered(&mut editor, branches());
    assert_eq!(editor.prompts().len(), 1, "only M-x's");
    assert!(last(&editor).contains("dropped"), "{}", last(&editor));
    assert!(!d.take_branches_request(), "nothing waits any more");
}

#[test]
fn an_error_listing_them_is_said_and_drops_the_wait() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-c s");
    d.take_branches_request();
    d.branches_answered(&mut editor, Err("Not in a git repository".into()));
    assert!(editor.prompt().is_none());
    assert_eq!(last(&editor), "Not in a git repository");
    d.branches_answered(&mut editor, branches());
    assert!(editor.prompt().is_none(), "the wait is gone");
}

#[test]
fn quitting_either_prompt_runs_nothing() {
    let (mut d, mut editor) = (dispatcher(), Editor::default());
    send(&mut d, &mut editor, "C-u C-c s");
    d.take_branches_request();
    d.branches_answered(&mut editor, branches());
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none());
    send(&mut d, &mut editor, "C-u C-c s");
    d.take_branches_request();
    d.branches_answered(&mut editor, branches());
    send(&mut d, &mut editor, "x RET C-g");
    assert!(editor.prompt().is_none());
    assert!(!last(&editor).starts_with("ran"), "{}", last(&editor));
}

#[test]
fn another_command_or_c_g_drops_the_wait() {
    for keys in ["C-g", "C-f", "x"] {
        let (mut d, mut editor) = (dispatcher(), Editor::default());
        send(&mut d, &mut editor, "C-c s");
        assert!(d.take_branches_request());
        send(&mut d, &mut editor, keys);
        assert!(
            kept_messages(&editor)
                .iter()
                .any(|m| m == "Branch prompt dropped"),
            "{keys}: {:?}",
            kept_messages(&editor)
        );
        d.branches_answered(&mut editor, branches());
        assert!(
            editor.prompt().is_none(),
            "{keys}: the late list opens nothing"
        );
    }
}
