//! Tests of the pull requests' commands: what each asks of the host from a status's project and
//! from a pull request's buffer, and refusals where they mean nothing.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::Args;
use oso_core::editor::{GitRequest, PrAct};

use super::KEYS;
use crate::commands::tests::run;

#[test]
fn the_menu_s_heads_ask_about_the_project_s_branch() {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    for name in [
        "git-pr-view",
        "git-pr-checks",
        "git-pr-list",
        "git-pr-browse",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let view = |checks| GitRequest::PrView {
        project,
        number: None,
        checks,
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            view(false),
            view(true),
            GitRequest::PrList { project },
            GitRequest::PrOpen { project },
        ]
    );
}

#[test]
fn a_pull_request_s_keys_act_at_the_primary_cursor() {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let buffer = editor.add_git_pr(project, "*pr: #12*");
    editor.show(buffer);
    for name in [
        "git-pr-visit",
        "git-pr-open",
        "git-pr-copy-link",
        "git-pr-refresh",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    let act = |act| GitRequest::PrAct {
        buffer,
        act,
        line: 0,
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            act(PrAct::Visit),
            act(PrAct::Open),
            act(PrAct::CopyLink),
            act(PrAct::Refresh)
        ]
    );
    // The menu's heads work from the pull request's buffer too, as of its project.
    let (result, _) = run(&mut editor, "git-pr-list", Args::NONE);
    assert!(result.is_ok());
}

#[test]
fn a_pull_request_s_keys_mean_nothing_elsewhere() {
    let (mut editor, _) = crate::commands::review::tests::in_project(true);
    let (result, _) = run(&mut editor, "git-pr-visit", Args::NONE);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Not in a pull request")
    );
    assert!(editor.take_git_requests().is_empty());
    assert!(
        KEYS.iter()
            .any(|(key, name)| *key == "RET" && *name == "git-pr-visit")
    );
}

#[test]
fn the_create_and_merge_heads_ask_with_their_switches() {
    use crate::commands::vcs::commit::tests::run_with;
    use oso_core::editor::MergeMethod;

    use super::create::{BASE, DELETE_BRANCH, DRAFT};
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    run_with(&mut editor, "git-pr-create", &[]).unwrap();
    run_with(&mut editor, "git-pr-create", &[DRAFT, BASE]).unwrap();
    run_with(&mut editor, "git-pr-merge-squash", &[DELETE_BRANCH]).unwrap();
    run_with(&mut editor, "git-pr-merge-rebase", &[]).unwrap();
    // A `y` counts only as a question's answer.
    let (result, _) = run(&mut editor, "git-pr-confirmed", Args::NONE);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No pull request was asked about")
    );
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::PrCreate {
                project,
                draft: false,
                ask_base: false
            },
            GitRequest::PrCreate {
                project,
                draft: true,
                ask_base: true
            },
            GitRequest::PrMerge {
                project,
                method: MergeMethod::Squash,
                delete_branch: true
            },
            GitRequest::PrMerge {
                project,
                method: MergeMethod::Rebase,
                delete_branch: false
            },
        ]
    );
}

#[test]
fn a_message_s_keys_act_in_a_message_only() {
    let (mut editor, _) = crate::commands::review::tests::in_project(true);
    let (result, _) = run(&mut editor, "git-pr-create-finish", Args::NONE);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Not in a pull request's message")
    );
    let mut buffer = oso_core::buffer::Buffer::from_text("*pull request: b*", "Title\n");
    buffer.set_mode(oso_core::buffer::Mode::GitPrMessage);
    let id = editor.add_buffer(buffer);
    editor.show(id);
    for name in [
        "git-pr-create-finish",
        "git-pr-create-cancel",
        "git-pr-previous-message",
        "git-pr-next-message",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::PrCreateFinish { buffer: id },
            GitRequest::PrCreateCancel { buffer: id },
            GitRequest::PrRing {
                buffer: id,
                older: true
            },
            GitRequest::PrRing {
                buffer: id,
                older: false
            },
        ]
    );
}
