//! `C-x C-c` over a native session's turn (ADR 0133, step 5; ADR 0165), with the composer's
//! fake `claude -p`, whose turn ends only once the flow writes its release file: while the
//! turn runs, the exit asks first and offers to wait, naming the session; once it has ended,
//! the exit goes, as an idle session loses nothing.
//!
//! Must not depend on: the app's internals; the question's text has its own tests.

use std::fs;

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::compose::COMPOSE_FAKE;
use super::{Flow, MESSAGE, in_composer, tracked};

/// A flow whose native session has its first turn under way, the fake holding it until
/// `release.1`, with the transcript selected.
fn turn_under_way(test: &str) -> (Flow, App) {
    let flow = Flow::new(test);
    let fake = flow.scratch.path().join("fake-claude");
    let script = COMPOSE_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert_eq!(flow.wait_for_file("sent.1").trim(), MESSAGE);
    app.wait_for("the turn under way", |state| tracked(state) == "working");
    (flow, app)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_c_c_asks_first_while_a_native_turn_runs() {
    let (flow, mut app) = turn_under_way("native-quit-turn");
    app.send_keys("C-x C-c");
    let state = app.wait_for("the exit's question", |state| {
        state["prompts"][0]["kind"] == "question"
    });
    let label = state["prompts"][0]["prompt"].as_str().unwrap_or_default();
    assert_eq!(
        label,
        "a turn is under way in *claude: proj*; exit anyway? (y, n or w)"
    );
    app.send_keys("n");
    assert!(!app.exited(), "the instance stays");
    // The turn ends: nothing is at risk any more, and the exit goes at once.
    flow.scratch.write("release.1", b"");
    app.wait_for("the turn's end", |state| {
        let tracked = tracked(state);
        !tracked.is_empty() && tracked != "working"
    });
    app.send_keys("C-x C-c");
    assert!(app.wait_for_exit().success(), "the instance exits");
}
