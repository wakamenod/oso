//! Tests of discarding's commands: what `k` asks of the host, as a key and as the question's
//! `y`, the list's keys, and the refusals outside them.
//!
//! Must not depend on: `app` or winit.

use oso_core::buffer::Mode;
use oso_core::command::Args;
use oso_core::editor::{GitLines, GitRequest};

use crate::commands::tests::{editor, run};

#[test]
fn k_asks_the_host_with_the_cursors_lines() {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let status = editor.add_git_status(project, "*git: p*");
    editor.show(status);
    let (result, _) = run(&mut editor, "git-discard", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::Discard {
            buffer: status,
            lines: vec![GitLines {
                first: 0,
                last: 0,
                region: false
            }],
            confirmed: false,
        }]
    );
    let (result, _) = run(&mut editor, "git-discarded", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::ShowDiscarded { buffer: status }]
    );
}

#[test]
fn ret_in_the_list_asks_to_write_its_line_back() {
    let mut editor = editor("");
    let list = editor.add_internal_buffer("*git discarded: p*", Mode::GitDiscarded);
    editor.set_internal_text(list, "Kept\n\n  just now  0123456  a.rs\n");
    editor.show(list);
    let (result, _) = run(&mut editor, "next-line", Args::NONE);
    assert!(result.is_ok());
    let _ = run(&mut editor, "next-line", Args::NONE);
    let (result, _) = run(&mut editor, "git-restore-discarded", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::RestoreDiscarded {
            buffer: list,
            line: 2,
            confirmed: false
        }]
    );
    let (result, _) = run(&mut editor, "git-discarded", Args::NONE);
    assert!(result.is_ok(), "g in the list reads it again");
}

#[test]
fn outside_their_buffers_the_commands_say_so() {
    let mut editor = editor("text");
    let (result, _) = run(&mut editor, "git-discard", Args::NONE);
    assert_eq!(result.unwrap_err(), "Not in a git status");
    let (result, _) = run(&mut editor, "git-restore-discarded", Args::NONE);
    assert_eq!(result.unwrap_err(), "Not in the list of what discards kept");
    let (result, _) = run(&mut editor, "git-discarded", Args::NONE);
    assert_eq!(result.unwrap_err(), "Not in a project");
    assert!(editor.take_git_requests().is_empty());
}
