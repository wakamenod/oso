//! Tests of the branch commands: what each head asks of the host, the prompts' answers, the
//! question's `y`, and refusals outside a status.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::Args;
use oso_core::editor::{BranchAction, Editor, GitRequest};

use super::{BRANCHES_LIST, STARTS_LIST, listed_in_given_order, listed_takes_input};
use crate::commands::tests::{editor, run};

fn in_status() -> (Editor, oso_core::buffer::BufferId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let id = editor.add_git_status(project, "*git: p*");
    editor.show(id);
    (editor, id)
}

#[test]
fn each_head_asks_for_its_action() {
    let (mut editor, buffer) = in_status();
    for name in [
        "git-branch-checkout",
        "git-branch-create-checkout",
        "git-branch-create",
        "git-branch-rename",
        "git-branch-delete",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let branch = |action| GitRequest::Branch { buffer, action };
    assert_eq!(
        editor.take_git_requests(),
        [
            branch(BranchAction::Checkout),
            branch(BranchAction::Create { checkout: true }),
            branch(BranchAction::Create { checkout: false }),
            branch(BranchAction::Rename),
            branch(BranchAction::Delete),
        ]
    );
}

#[test]
fn the_question_s_command_runs_only_as_its_y() {
    let (mut editor, _) = in_status();
    let (result, _) = run(&mut editor, "git-branch-delete-confirmed", Args::NONE);
    assert!(result.is_err());
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn outside_a_status_the_heads_say_so() {
    let mut editor = editor("text\n");
    for name in ["git-branch-checkout", "git-branch-delete"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_err(), "{name}");
    }
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn only_the_start_s_prompt_takes_what_is_typed_and_both_keep_their_order() {
    assert!(listed_takes_input(STARTS_LIST));
    assert!(!listed_takes_input(BRANCHES_LIST));
    assert!(listed_in_given_order(STARTS_LIST) && listed_in_given_order(BRANCHES_LIST));
}
