//! `oso-restart` waits for a native session's turn (ADR 0165, part 4b), with the composer's
//! fake `claude -p`, whose turn ends only once the flow writes its release file: the restart
//! starts no instance while the turn runs, and once the session has been idle for the hold,
//! the new instance takes over; `C-g` cancels a restart that waits.
//!
//! Must not depend on: the app's internals; what holds the restart, the echo and the cancel
//! have their own tests.

use std::fs;
use std::time::Duration;

use super::super::super::harness::{App, sockets};
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::compose::COMPOSE_FAKE;
use super::{Flow, MESSAGE, in_composer, tracked};

/// How long a flow watches that no instance starts: past the 2 s hold.
const WATCHED: Duration = Duration::from_secs(3);

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

/// Runs `oso-restart`, which waits for the session, and returns the sockets there were before.
fn restart_waits(app: &mut App) -> Vec<u32> {
    let before = sockets();
    app.run_command("oso-restart", None)
        .expect("oso-restart runs");
    app.wait_for("the restart to wait", |state| {
        state["echo"].as_str().is_some_and(|echo| {
            echo.contains("oso-restart waits for *claude: proj* to be idle; C-g cancels")
        })
    });
    before
}

/// Asserts that no instance starts for [`WATCHED`].
fn no_instance_starts(before: &[u32]) {
    std::thread::sleep(WATCHED);
    let new: Vec<u32> = (sockets().into_iter())
        .filter(|pid| !before.contains(pid))
        .collect();
    assert!(new.is_empty(), "an instance started: {new:?}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn oso_restart_waits_for_a_native_turn_then_restarts() {
    let (flow, mut old) = turn_under_way("native-restart-wait");
    let before = restart_waits(&mut old);
    no_instance_starts(&before);
    // The turn ends: once the session has been idle for the hold, the restart begins.
    flow.scratch.write("release.1", b"");
    let mut new = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    new.wait_until_ready();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_g_cancels_a_restart_that_waits_for_a_native_turn() {
    let (flow, mut app) = turn_under_way("native-restart-wait-cancel");
    let before = restart_waits(&mut app);
    app.send_keys("C-g");
    app.wait_for("the wait cancelled", |state| {
        state["echo"] == "Restart when idle cancelled"
    });
    flow.scratch.write("release.1", b"");
    app.wait_for("the turn's end", |state| {
        let tracked = tracked(state);
        !tracked.is_empty() && tracked != "working"
    });
    no_instance_starts(&before);
    assert!(!app.exited(), "the instance stays");
}
