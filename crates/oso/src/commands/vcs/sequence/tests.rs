//! Tests of the commands of what is stopped: what each asks of the host, the questions' `y`
//! commands refused when run on their own, and refusals outside a status.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::Args;
use oso_core::editor::{Editor, GitRequest};

use crate::commands::tests::{editor, run};

fn in_status() -> (Editor, oso_core::buffer::BufferId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let id = editor.add_git_status(project, "*git: p*");
    editor.show(id);
    (editor, id)
}

#[test]
fn each_key_asks_for_its_step_unconfirmed() {
    let (mut editor, buffer) = in_status();
    for name in ["git-continue", "git-skip", "git-abort"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_ok(), "{name}");
    }
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::Continue { buffer },
            GitRequest::Skip {
                buffer,
                confirmed: false,
            },
            GitRequest::Abort {
                buffer,
                confirmed: false,
            },
        ]
    );
}

#[test]
fn the_questions_commands_run_only_as_their_y() {
    let (mut editor, _) = in_status();
    for name in ["git-skip-confirmed", "git-abort-confirmed"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_err(), "{name}");
    }
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn outside_a_status_they_say_so() {
    let mut editor = editor("text\n");
    for name in ["git-continue", "git-skip", "git-abort"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_err(), "{name}");
    }
    assert!(editor.take_git_requests().is_empty());
}
