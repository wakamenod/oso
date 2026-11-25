//! The windows on native sessions across a real `oso-restart` (ADR 0165, part 4c-2), with the
//! modes' fake: every window of the tree that showed a transcript shows it again, at its end, its
//! composer selected if it was, and the space of the session shown is in front; a stopped session's
//! transcript comes back too, and a window on a file stays on it.
//!
//! Must not depend on: the app's internals; what the state says, what stays the person's and the
//! layouts that do not fit have their own tests.

use std::fs;

use serde_json::Value;

use super::super::super::harness::{App, sockets};
use super::super::super::spaces::{switch_space, trust_restricted};
use super::super::super::worktrees::keys;
use super::modes::{modeline, send};
use super::restart::idle_in_accept_edits;
use super::{Flow, composer_docked, git_init, in_composer, selected_composer, text};

/// The windows of the tree as the state lists them, the side and docked ones apart: what buffer
/// each shows, and where.
fn tree(state: &Value) -> Vec<(Value, Value)> {
    (state["windows"].as_array().into_iter().flatten())
        .filter(|window| window["docked"].is_null() && window["id"] != state["sessions"]["side"])
        .map(|window| (window["buffer"].clone(), window["area"].clone()))
        .collect()
}

/// The selected window's text, whatever wraps it.
fn bare(state: &Value) -> String {
    text(state).split_whitespace().collect()
}

/// Which of the tree's windows is selected.
fn selected_at(state: &Value) -> Option<usize> {
    let selected = (state["windows"].as_array().into_iter().flatten())
        .find(|window| window["selected"] == true)?;
    (tree(state).iter()).position(|(_, area)| *area == selected["area"])
}

/// Which of the tree's windows show the transcript whose composer is selected.
fn on_transcript(state: &Value) -> Vec<bool> {
    let host = selected_composer(state).map(|composer| &composer["docked"]["host"]);
    (tree(state).iter())
        .map(|(buffer, _)| Some(buffer) == host)
        .collect()
}

/// Restarts `old` into a new instance. `old` outlives it: its log's reader ends with both.
fn restarted(old: &mut App) -> App {
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
    new
}

/// Starts a session of a second repository, `other`, beside the flow's, in a space of its own, and
/// brings that to the front.
fn other_in_front(flow: &Flow, app: &mut App) {
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    git_init(&other);
    fs::write(other.join("other.md"), "other\n").unwrap();
    app.open_file(&other.join("other.md"));
    trust_restricted(app, "other");
    app.wait_for("the other project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the other composer", in_composer);
    send(app, "first");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "first");
    switch_space(app, "other");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn windows_on_a_transcript_come_back_with_the_composer_and_the_space_in_front() {
    let (flow, mut old) = idle_in_accept_edits("native-restart-windows", None);
    other_in_front(&flow, &mut old);
    // A file at the left, the transcript twice at the right, the lower one's composer selected.
    old.send_keys("C-x 1");
    old.wait_for("one window", |state| tree(state).len() == 1);
    old.open_file(&flow.scratch.path().join("other").join("other.md"));
    old.wait_for("the file", |state| state["buffer"]["name"] == "other.md");
    // A split selects the new window.
    old.send_keys("C-x 3");
    old.wait_for("the right window", |state| selected_at(state) == Some(1));
    old.send_keys("C-x b");
    old.send_keys(&keys("*claude: other*"));
    old.send_keys("RET");
    old.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: other*"
    });
    old.send_keys("C-x 2 i");
    let before = old.wait_for("the layout", |state| {
        composer_docked(state) && on_transcript(state) == [false, true, true]
    });
    let areas =
        |state: &Value| -> Vec<Value> { tree(state).into_iter().map(|(_, area)| area).collect() };
    let mut new = restarted(&mut old);
    let after = new.wait_for("the layout again", |state| {
        composer_docked(state) && on_transcript(state) == [false, true, true]
    });
    assert_eq!(areas(&after), areas(&before));
    let host = |state: &Value| {
        let composer = selected_composer(state).expect("a composer");
        composer["area"].clone()
    };
    assert_eq!(host(&after), host(&before), "under the same window");
    new.wait_for("the space in front", |state| {
        state["sessions"]["space"] == "other"
    });
    // At its end, and it follows what comes.
    // Both sessions resume at once, and their fakes may take one number.
    flow.wait_for_file("run.3");
    new.send_keys("C-c C-o");
    new.wait_for("the row of its resume", |state| {
        state["buffer"]["name"] == "*claude: other*"
            && bare(state).contains("Resumedafteroso-restart.")
    });
    new.send_keys("i");
    new.wait_for("the composer", in_composer);
    send(&mut new, "second");
    new.send_keys("C-c C-o");
    new.wait_for("its reply", |state| {
        state["buffer"]["name"] == "*claude: other*" && text(state).contains("reply")
    });
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_stopped_session_comes_back_in_its_window_with_the_transcript_selected() {
    let (flow, mut old) = idle_in_accept_edits("native-restart-window-stopped", None);
    old.send_keys("C-c C-o");
    old.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    old.run_command("session-stop", None).unwrap();
    old.wait_for("the session stopped", |state| {
        modeline(state).contains("stopped")
    });
    // The other half on the file.
    old.send_keys("C-x 2");
    old.wait_for("two windows", |state| tree(state).len() == 2);
    old.open_file(&flow.project.join("notes.md"));
    old.wait_for("the file", |state| state["buffer"]["name"] == "notes.md");
    old.send_keys("C-x o");
    old.wait_for("the transcript selected", |state| {
        state["buffer"]["name"] == "*claude: proj*" && tree(state).len() == 2
    });
    let mut new = restarted(&mut old);
    let after = new.wait_for("the transcript selected again", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert_eq!(tree(&after).len(), 2);
    assert!(!in_composer(&after));
    let shows = tree(&after)
        .iter()
        .filter(|(buffer, _)| *buffer == after["buffer"]["id"])
        .count();
    assert_eq!(shows, 1, "the file's window stays on it");
    assert!(text(&after).contains("Stopped."), "{after}");
    assert!(
        !flow.scratch.path().join("run.2").exists(),
        "nothing resumed"
    );
    let _ = new.exchange("exit", None);
}
