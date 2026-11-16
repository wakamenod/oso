//! Tests of the remotes' commands: what each head asks of the host with the switches on, the
//! prompts' answers, the question's `y`, and refusals outside a status.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::Args;
use oso_core::editor::{Editor, FetchFrom, GitRequest, PushSwitches, PushTarget};

use super::listed_takes_input;
use super::{FORCE_WITH_LEASE, PULL_MERGE, PULL_REBASE, PUSH_NO_VERIFY, TARGETS_LIST};
use crate::commands::tests::{editor, run};
use crate::commands::vcs::commit::tests::run_with;

fn in_status() -> (Editor, oso_core::buffer::BufferId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let id = editor.add_git_status(project, "*git: p*");
    editor.show(id);
    (editor, id)
}

#[test]
fn each_push_head_asks_for_its_target_with_the_switches_on() {
    let (mut editor, buffer) = in_status();
    run_with(&mut editor, "git-push", &[FORCE_WITH_LEASE]).unwrap();
    run_with(&mut editor, "git-push-upstream", &[PUSH_NO_VERIFY]).unwrap();
    run_with(&mut editor, "git-push-elsewhere", &[]).unwrap();
    let push = |target, switches| GitRequest::Push {
        buffer,
        target,
        switches,
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            push(
                PushTarget::PushRemote,
                PushSwitches {
                    force_with_lease: true,
                    no_verify: false,
                }
            ),
            push(
                PushTarget::Upstream,
                PushSwitches {
                    force_with_lease: false,
                    no_verify: true,
                }
            ),
            push(PushTarget::Elsewhere, PushSwitches::default()),
        ]
    );
}

#[test]
fn pull_and_fetch_ask_for_their_kind() {
    let (mut editor, buffer) = in_status();
    for name in ["git-pull", "git-fetch", "git-fetch-all"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::Pull {
                buffer,
                how: oso_core::editor::PullHow::FfOnly,
            },
            GitRequest::Fetch {
                buffer,
                from: FetchFrom::Upstream,
            },
            GitRequest::Fetch {
                buffer,
                from: FetchFrom::All,
            },
        ]
    );
}

#[test]
fn the_pull_switches_choose_how_and_both_are_refused() {
    use oso_core::editor::PullHow;
    let (mut editor, buffer) = in_status();
    run_with(&mut editor, "git-pull", &[PULL_REBASE]).unwrap();
    run_with(&mut editor, "git-pull", &[PULL_MERGE]).unwrap();
    assert!(run_with(&mut editor, "git-pull", &[PULL_REBASE, PULL_MERGE]).is_err());
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::Pull {
                buffer,
                how: PullHow::Rebase,
            },
            GitRequest::Pull {
                buffer,
                how: PullHow::Merge,
            },
        ]
    );
}

#[test]
fn the_question_s_command_runs_only_as_its_y() {
    let (mut editor, _) = in_status();
    let (result, _) = run(&mut editor, "git-push-confirmed", Args::NONE);
    assert!(result.is_err());
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn outside_a_status_the_heads_say_so() {
    let mut editor = editor("text\n");
    for name in ["git-push", "git-pull", "git-fetch-all"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_err(), "{name}");
    }
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn only_the_push_targets_prompt_takes_what_is_typed() {
    assert!(listed_takes_input(TARGETS_LIST));
    assert!(!listed_takes_input(super::REMOTES_LIST));
}
