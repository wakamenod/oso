//! Scenario tests of `trust-project` and `untrust-project` run by keys (ADR 0114): the
//! question, the `y` that re-checks it, and the refusals.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::path::PathBuf;

use oso_core::buffer::{Baseline, Buffer};
use oso_core::editor::{Editor, Status};
#[cfg(unix)]
use oso_core::project::TrustChange;
use oso_core::project::{Found, RootIdentity, TrustList};
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::motion_tests::send;

/// An editor on a file of the project `/home/me/src/oso`, its record of trusted projects
/// read as `record`.
fn in_oso(record: Option<&str>) -> (Dispatcher, Editor) {
    let mut buffer = Buffer::visiting(PathBuf::from("/home/me/src/oso/a.rs"));
    buffer.set_baseline(Baseline {
        location: PathBuf::from("/home/me/src/oso/a.rs"),
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(24, 120));
    editor.set_home(Some(PathBuf::from("/home/me")));
    let id = editor.tiled().0.buffer();
    editor.request_project(id);
    let request = editor.take_project_requests().pop().unwrap();
    let found = Found {
        root: RootIdentity {
            path: PathBuf::from("/home/me/src/oso"),
            identity: cfg!(unix).then_some((1, 7)),
        },
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
    editor.set_trust_list(record.map(|text| TrustList::parse(text).0));
    (Dispatcher::new(), editor)
}

/// Runs `command` by name, from `M-x`, then sends `then`.
fn m_x(d: &mut Dispatcher, editor: &mut Editor, command: &str, then: &str) {
    let typed: Vec<String> = command.chars().map(String::from).collect();
    send(d, editor, &format!("M-x {} RET {then}", typed.join(" ")));
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(&mut status);
    status.echo
}

#[cfg(unix)]
fn trusted(editor: &Editor) -> bool {
    let id = editor.tiled().1.project().unwrap();
    editor.project_trusted(id)
}

#[cfg(unix)]
#[test]
fn trust_project_asks_naming_the_root_and_y_trusts_it() {
    let (mut d, mut editor) = in_oso(Some(""));
    m_x(&mut d, &mut editor, "trust-project", "");
    let question = editor.prompt().unwrap().label().to_owned();
    assert_eq!(
        question,
        "Trust ~/src/oso? Its code may run through language servers, formatters and git hooks \
         (y or n)"
    );
    send(&mut d, &mut editor, "n");
    assert!(!trusted(&editor), "n keeps it restricted");
    assert_eq!(editor.take_trust_changes(), []);
    m_x(&mut d, &mut editor, "trust-project", "y");
    assert!(trusted(&editor));
    assert_eq!(echo(&editor), "Trusted ~/src/oso");
    let root = RootIdentity {
        path: PathBuf::from("/home/me/src/oso"),
        identity: Some((1, 7)),
    };
    assert_eq!(editor.take_trust_changes(), [TrustChange::Trust(root)]);
    m_x(&mut d, &mut editor, "trust-project", "");
    assert!(editor.prompt().is_none());
    assert_eq!(echo(&editor), "~/src/oso is trusted already");
}

/// Only a `y` to the question confirms: run again after `C-g`, the command asks again.
#[cfg(unix)]
#[test]
fn only_a_y_to_the_question_trusts_and_a_new_run_asks_again() {
    let (mut d, mut editor) = in_oso(Some(""));
    m_x(&mut d, &mut editor, "trust-project", "C-g");
    m_x(&mut d, &mut editor, "trust-project", "");
    assert!(
        editor
            .prompt()
            .unwrap()
            .label()
            .starts_with("Trust ~/src/oso?")
    );
    assert!(!trusted(&editor));
    assert_eq!(editor.take_trust_changes(), []);
}

#[cfg(unix)]
#[test]
fn untrust_project_takes_the_line_out_without_asking() {
    let (mut d, mut editor) = in_oso(Some("/home/me/src/oso\tdev=1 ino=7\n"));
    assert!(trusted(&editor));
    m_x(&mut d, &mut editor, "untrust-project", "");
    assert!(editor.prompt().is_none());
    assert!(!trusted(&editor));
    assert_eq!(echo(&editor), "~/src/oso is restricted again");
    let untrust = TrustChange::Untrust(PathBuf::from("/home/me/src/oso"));
    assert_eq!(editor.take_trust_changes(), [untrust]);
    assert_eq!(editor.trust_list(), Some(&TrustList::default()));
    m_x(&mut d, &mut editor, "untrust-project", "");
    assert_eq!(echo(&editor), "~/src/oso is not trusted");
}

/// A `y` given after the buffer shown went to another project asks about that one instead.
#[cfg(unix)]
#[test]
fn a_yes_after_the_buffer_changed_project_asks_again_naming_the_new_root() {
    let (mut d, mut editor) = in_oso(Some(""));
    m_x(&mut d, &mut editor, "trust-project", "");
    assert!(editor.prompt().is_some());
    // The question waits; meanwhile the buffer beneath it is found in another project.
    let id = editor.tiled().0.buffer();
    let other = PathBuf::from("/home/me/src/other/a.rs");
    editor.buffer_mut(id).unwrap().set_baseline(Baseline {
        location: other.clone(),
        file: None,
    });
    editor.request_project(id);
    let request = editor.take_project_requests().pop().unwrap();
    let found = Found {
        root: RootIdentity {
            path: PathBuf::from("/home/me/src/other"),
            identity: Some((1, 9)),
        },
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
    send(&mut d, &mut editor, "y");
    assert!(!trusted(&editor));
    assert_eq!(editor.take_trust_changes(), []);
    let question = editor.prompt().unwrap().label().to_owned();
    assert!(question.starts_with("Trust ~/src/other?"), "{question}");
}

#[test]
fn both_refuse_in_a_buffer_with_no_project_or_with_no_record_read() {
    let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
    editor.set_view_size(ViewSize::new(24, 120));
    let mut d = Dispatcher::new();
    for command in ["trust-project", "untrust-project"] {
        m_x(&mut d, &mut editor, command, "");
        assert!(editor.prompt().is_none(), "{command}");
        assert_eq!(echo(&editor), "Not in a project", "{command}");
    }
    let (mut d, mut editor) = in_oso(None);
    for command in ["trust-project", "untrust-project"] {
        m_x(&mut d, &mut editor, command, "");
        assert!(editor.prompt().is_none(), "{command}");
        assert!(
            echo(&editor).starts_with("trusted-projects is not read"),
            "{command}"
        );
    }
    assert_eq!(editor.take_trust_changes(), []);
}
