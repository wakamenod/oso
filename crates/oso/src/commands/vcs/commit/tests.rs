//! Tests of committing's commands: what each head asks of the host with the switches its menu
//! had on, the keys of a commit's message, the log's stop, `C-g` in a status, and the
//! refusals outside them.
//!
//! Must not depend on: `app` or winit.

use oso_core::buffer::{Buffer, Mode};
use oso_core::command::{Args, Ctx, ExecuteError, Invocation};
use oso_core::editor::{CommitKind, CommitSwitches, Editor, GitRequest};

use crate::commands::registry;
use crate::commands::tests::{editor, run};

/// Runs `name` as a head of a menu whose `switches` are on.
pub(in crate::commands::vcs) fn run_with(
    editor: &mut Editor,
    name: &str,
    switches: &[&str],
) -> Result<(), String> {
    let registry = registry();
    let id = registry.lookup(name).unwrap();
    let invocation = Invocation {
        switches,
        ..Invocation::default()
    };
    let mut ctx = Ctx::invoked(editor, invocation);
    registry
        .execute(id, &mut ctx, Args::NONE)
        .map_err(|e| match e {
            ExecuteError::Failed { error, .. } => error.message().to_owned(),
            other => panic!("{other}"),
        })
}

/// An editor showing a status buffer.
fn in_status() -> (Editor, oso_core::buffer::BufferId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let id = editor.add_git_status(project, "*git: p*");
    editor.show(id);
    (editor, id)
}

#[test]
fn each_head_asks_for_its_kind_with_the_switches_on() {
    let (mut editor, status) = in_status();
    run_with(&mut editor, "git-commit", &["no-verify", "allow-empty"]).unwrap();
    for name in ["git-commit-amend", "git-commit-extend", "git-commit-reword"] {
        run_with(&mut editor, name, &["signoff"]).unwrap();
    }
    let signoff = CommitSwitches {
        signoff: true,
        ..CommitSwitches::default()
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::Commit {
                buffer: status,
                kind: CommitKind::New,
                switches: CommitSwitches {
                    no_verify: true,
                    signoff: false,
                    allow_empty: true,
                },
                line: 0,
            },
            GitRequest::Commit {
                buffer: status,
                kind: CommitKind::Amend,
                switches: signoff,
                line: 0,
            },
            GitRequest::Commit {
                buffer: status,
                kind: CommitKind::Extend,
                switches: signoff,
                line: 0,
            },
            GitRequest::Commit {
                buffer: status,
                kind: CommitKind::Reword,
                switches: signoff,
                line: 0,
            },
        ]
    );
    // From `M-x`, no switch is on.
    let (result, _) = run(&mut editor, "git-commit", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::Commit {
            buffer: status,
            kind: CommitKind::New,
            switches: CommitSwitches::default(),
            line: 0,
        }]
    );
}

#[test]
fn fixup_and_squash_name_the_line_point_is_on() {
    let (mut editor, status) = in_status();
    let text = "Head:     main\n\nRecent commits (2)\nabc first\ndef second\n";
    editor.set_git_text(
        status,
        oso_core::load::decode_all(text.as_bytes(), Default::default()),
        oso_core::editor::GitShown {
            highlights: oso_core::syntax::Highlights::new(text.len()),
            outline: vec![oso_core::editor::GitHeading { line: 2, depth: 0 }],
            line: 4,
            lines: Vec::new(),
        },
    );
    run_with(&mut editor, "git-commit-fixup", &["no-verify"]).unwrap();
    run_with(&mut editor, "git-commit-squash", &[]).unwrap();
    let no_verify = CommitSwitches {
        no_verify: true,
        ..CommitSwitches::default()
    };
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::Commit {
                buffer: status,
                kind: CommitKind::Fixup,
                switches: no_verify,
                line: 4,
            },
            GitRequest::Commit {
                buffer: status,
                kind: CommitKind::Squash,
                switches: CommitSwitches::default(),
                line: 4,
            },
        ]
    );
}

#[test]
fn a_messages_keys_ask_for_it() {
    let mut editor = editor("Subject\n");
    let mut buffer = Buffer::from_text("*commit: p*", "Subject\n");
    assert!(buffer.set_mode(Mode::GitCommit));
    let id = editor.add_buffer(buffer);
    editor.show(id);
    for name in [
        "git-commit-finish",
        "git-commit-cancel",
        "git-commit-previous-message",
        "git-commit-next-message",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::CommitFinish { buffer: id },
            GitRequest::CommitCancel { buffer: id },
            GitRequest::CommitRing {
                buffer: id,
                older: true
            },
            GitRequest::CommitRing {
                buffer: id,
                older: false
            },
        ]
    );
}

#[test]
fn the_log_and_the_status_stop_what_git_runs_and_c_g_does_too() {
    let (mut editor, status) = in_status();
    let (result, _) = run(&mut editor, "git-show-log", Args::NONE);
    assert!(result.is_ok());
    let (result, _) = run(&mut editor, "git-stop", Args::NONE);
    assert!(result.is_ok());
    let (result, _) = run(&mut editor, "keyboard-quit", Args::NONE);
    assert!(result.is_ok());
    let log = editor.add_internal_buffer("*git log: p*", Mode::GitLog);
    editor.show(log);
    let (result, _) = run(&mut editor, "git-stop", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::ShowLog { buffer: status },
            GitRequest::Stop { buffer: status },
            GitRequest::Stop { buffer: status },
            GitRequest::Stop { buffer: log },
        ]
    );
}

#[test]
fn outside_their_buffers_the_commands_say_so() {
    let mut editor = editor("text");
    for (name, said) in [
        ("git-commit", "Not in a git status"),
        ("git-show-log", "Not in a git status"),
        ("git-commit-finish", "Not in a commit's message"),
        ("git-commit-previous-message", "Not in a commit's message"),
        ("git-stop", "Not in a git status or its log"),
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert_eq!(result.unwrap_err(), said, "{name}");
    }
    // `C-g` elsewhere asks nothing of git.
    let (result, _) = run(&mut editor, "keyboard-quit", Args::NONE);
    assert!(result.is_ok());
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn quitting_names_git_running_and_quit_when_idle_counts_it() {
    let mut editor = editor("");
    let log = editor.add_internal_buffer("*git log: p*", Mode::GitLog);
    editor.set_git_running(log, true);
    let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
    assert!(result.is_ok());
    assert!(!effects.exit);
    assert_eq!(
        editor.prompt().expect("a question").label(),
        "git runs in *git log: p*; exit anyway? (y, n or w)"
    );
    editor.quit();
    let (result, _) = run(&mut editor, "quit-when-idle", Args::NONE);
    assert!(result.is_ok());
    assert!(editor.quit_when_idle());
    editor.set_git_running(log, false);
    assert_eq!(editor.git_runs().count(), 0);
}

#[test]
fn removing_a_stale_lock_asks_the_host_in_a_project_or_a_log_only() {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let status = editor.add_git_status(project, "*git: p*");
    editor.show(status);
    let (result, _) = run(&mut editor, "git-remove-stale-lock", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::RemoveLock {
            buffer: status,
            confirmed: false
        }]
    );
    let log = editor.add_internal_buffer("*git log: p*", Mode::GitLog);
    editor.show(log);
    let (result, _) = run(&mut editor, "git-remove-stale-lock", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::RemoveLock {
            buffer: log,
            confirmed: false
        }]
    );
    let mut editor = crate::commands::tests::editor("");
    let (result, _) = run(&mut editor, "git-remove-stale-lock", Args::NONE);
    assert_eq!(result.unwrap_err().to_string(), "Not in a project");
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn a_message_that_a_secret_was_yanked_into_is_not_committed() {
    let mut editor = editor("");
    let mut buffer = Buffer::from_text("*commit: p*", "Subject\n");
    assert!(buffer.set_mode(Mode::GitCommit));
    let id = editor.add_buffer(buffer);
    editor.show(id);
    editor.kill_new("hunter2", true);
    editor.yank(0, false).unwrap();
    assert!(editor.current().1.is_secret());
    let (result, _) = run(&mut editor, "git-commit-finish", Args::NONE);
    assert_eq!(
        result.unwrap_err(),
        "The message holds secret text, which is not committed: C-c C-k drops it"
    );
    // It can still be dropped: the host keeps no secret message in the ring.
    let (result, _) = run(&mut editor, "git-commit-cancel", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(
        editor.take_git_requests(),
        [GitRequest::CommitCancel { buffer: id }]
    );
}
