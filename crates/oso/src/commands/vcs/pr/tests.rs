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
