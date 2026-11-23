//! Scenario tests of an exit that waits for the terminals, with its keys, through the
//! [`Dispatcher`] (ADR 0133, step 5): `C-x C-c`'s third answer, the exit once idle, and what
//! the person agreed to lose meanwhile.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;

use super::Dispatcher;

const QUIT: &str = "C-x C-c";

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) -> Effects {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0)
}

/// The question the innermost prompt asks, if it is one.
fn question(editor: &Editor) -> Option<&str> {
    editor
        .prompt()
        .filter(|prompt| prompt.kind().is_question())
        .map(|prompt| prompt.label())
}

#[test]
fn exit_with_a_job_running_waits_on_w_and_goes_once_idle() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    let shell = editor.add_terminal_buffer("*terminal*");
    editor.set_running(shell, true);
    editor.set_terminal_job(shell, true);
    assert!(!send(&mut dispatcher, &mut editor, QUIT).exit);
    assert_eq!(
        question(&editor),
        Some(
            "a job runs in *terminal*, and 1 modified buffer; exit without saving? \
             (y, n or w)"
        )
    );
    // `w` waits, and nothing exits while the job runs.
    assert!(!send(&mut dispatcher, &mut editor, "w").exit);
    assert!(editor.quit_when_idle());
    // Idle, the exit goes, as the person agreed to lose the modified buffer.
    editor.set_terminal_job(shell, false);
    assert!(dispatcher.exit_when_idle(&mut editor).unwrap().exit);
}

/// What the echo area says.
fn echo(editor: &Editor) -> String {
    let mut status = oso_core::editor::Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

/// An editor with a modified buffer and a shell running a job, and the shell.
fn modified_with_a_job() -> (Dispatcher, Editor, oso_core::buffer::BufferId) {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    let shell = editor.add_terminal_buffer("*terminal*");
    editor.set_running(shell, true);
    editor.set_terminal_job(shell, true);
    (dispatcher, editor, shell)
}

/// Whether the idle exit asked nothing and went nowhere, saying why.
fn stayed_saying_why(dispatcher: &mut Dispatcher, editor: &mut Editor) -> bool {
    let effects = dispatcher.exit_when_idle(editor).unwrap();
    !effects.exit
        && editor.prompt().is_none()
        && echo(editor).starts_with("Not quit, as more is at risk now: 1 modified buffer")
}

#[test]
fn an_exit_once_idle_asks_nothing_for_more_at_risk_and_waits_for_a_prompt() {
    let (mut dispatcher, mut editor, shell) = modified_with_a_job();
    send(&mut dispatcher, &mut editor, QUIT);
    send(&mut dispatcher, &mut editor, "w");
    send(&mut dispatcher, &mut editor, "C-x b");
    assert!(
        dispatcher.exit_when_idle(&mut editor).is_none(),
        "a prompt is open"
    );
    send(&mut dispatcher, &mut editor, "C-g");
    // Edited since `w`: the text the question named is not the text there now.
    send(&mut dispatcher, &mut editor, "y");
    editor.set_terminal_job(shell, false);
    assert!(stayed_saying_why(&mut dispatcher, &mut editor));
    // And the next `C-x C-c` asks again.
    send(&mut dispatcher, &mut editor, QUIT);
    assert!(question(&editor).is_some());
}

#[test]
fn a_wait_cancelled_or_asked_for_by_name_agrees_to_nothing() {
    let (mut dispatcher, mut editor, shell) = modified_with_a_job();
    send(&mut dispatcher, &mut editor, QUIT);
    send(&mut dispatcher, &mut editor, "w C-g");
    assert!(!editor.quit_when_idle());
    dispatcher
        .run_command(&mut editor, "quit-when-idle", None)
        .1
        .unwrap();
    assert!(editor.quit_when_idle());
    editor.set_terminal_job(shell, false);
    assert!(stayed_saying_why(&mut dispatcher, &mut editor));
}

#[test]
fn w_answers_only_a_question_that_has_a_third_answer() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    send(&mut dispatcher, &mut editor, "x");
    send(&mut dispatcher, &mut editor, QUIT);
    send(&mut dispatcher, &mut editor, "w");
    assert!(editor.prompt().is_some());
    assert!(!editor.quit_when_idle());
}

#[test]
fn an_exit_once_idle_stops_for_a_comment_not_sent() {
    use crate::commands::review::tests::{comment_in, in_project, with_review};
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    comment_in(&mut editor, review);
    let mut dispatcher = Dispatcher::new();
    let shell = editor.add_terminal_buffer("*terminal*");
    editor.set_running(shell, true);
    editor.set_terminal_job(shell, true);
    send(&mut dispatcher, &mut editor, QUIT);
    assert!(question(&editor).is_some_and(|q| q.starts_with("1 comment of yours will be lost")));
    send(&mut dispatcher, &mut editor, "w");
    assert!(editor.quit_when_idle());
    editor.set_terminal_job(shell, false);
    let effects = dispatcher.exit_when_idle(&mut editor).unwrap();
    assert!(!effects.exit);
    assert!(editor.prompt().is_none());
    assert_eq!(
        echo(&editor),
        "Not quit: 1 comment of yours not sent; C-x C-c asks"
    );
}

#[test]
fn c_g_in_a_native_session_or_c_x_c_c_cancels_a_restart_that_waits() {
    let (mut dispatcher, mut editor) = (Dispatcher::new(), Editor::default());
    let transcript = editor.add_native_session_buffer("*claude: s*");
    editor.show(transcript);
    // The host waits for the native sessions (ADR 0165, part 4b).
    editor.set_restart_when_idle(true);
    send(&mut dispatcher, &mut editor, "C-g");
    assert!(!editor.restart_when_idle());
    assert_eq!(echo(&editor), "Restart when idle cancelled");
    editor.set_restart_when_idle(true);
    send(&mut dispatcher, &mut editor, QUIT);
    assert!(!editor.restart_when_idle());
}
