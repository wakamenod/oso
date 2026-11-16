//! Tests of the stash commands: what each head asks of the host with point's line and the switch
//! on, the prompts' answers, the question's `y`, and refusals outside a status.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::Args;
use oso_core::editor::{Editor, GitRequest, StashAction};

use super::STASH_UNTRACKED;
use crate::commands::tests::{editor, run};
use crate::commands::vcs::commit::tests::run_with;

fn in_status() -> (Editor, oso_core::buffer::BufferId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let id = editor.add_git_status(project, "*git: p*");
    editor.show(id);
    (editor, id)
}

#[test]
fn each_head_asks_for_its_action_at_point() {
    let (mut editor, buffer) = in_status();
    run_with(&mut editor, "git-stash", &[STASH_UNTRACKED]).unwrap();
    for name in ["git-stash-apply", "git-stash-pop", "git-stash-drop"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let stash = |action| GitRequest::Stash {
        buffer,
        action,
        line: 0,
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            stash(StashAction::Push { untracked: true }),
            stash(StashAction::Apply),
            stash(StashAction::Pop),
            stash(StashAction::Drop),
        ]
    );
}

#[test]
fn the_question_s_command_runs_only_as_its_y() {
    let (mut editor, _) = in_status();
    let (result, _) = run(&mut editor, "git-stash-drop-confirmed", Args::NONE);
    assert!(result.is_err());
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn outside_a_status_the_heads_say_so() {
    let mut editor = editor("text\n");
    for name in ["git-stash", "git-stash-apply", "git-stash-drop"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_err(), "{name}");
    }
    assert!(editor.take_git_requests().is_empty());
}
