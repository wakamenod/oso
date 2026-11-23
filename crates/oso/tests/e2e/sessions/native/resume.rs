//! A native session's resume and fork (ADR 0165, step 2, R1 and Q2 (a)), with a fake
//! `claude -p` that logs each run's arguments and answers each message at once: a stopped
//! session resumes natively in its own buffer, keeping its rows, with the composer selected;
//! a fork of it is native in a new buffer.
//!
//! Must not depend on: the app's internals.

use std::fs;

use serde_json::Value;

use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, end_turn, in_composer, text};

/// The fake: run `N` from 1 logs its arguments to `run.N`, and for each line of its input
/// replays it and replies `reply N.M`: at once, but the first, which ends once the flow writes
/// `release.1.1`. It exits once its input ends.
pub(super) const RESUME_FAKE: &str = r#"#!/bin/sh
dir='DIR'
n=$(ls "$dir" | grep -c '^run\.')
n=$((n+1))
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$n"
mv "$dir/tmp.$n" "$dir/run.$n"
cat "$dir/stream.jsonl"
m=0
while IFS= read -r line; do
  case "$line" in *'"control_request"'*) continue ;; esac
  m=$((m+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n.$m"
  printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
  if [ "$n.$m" = 1.1 ]; then
    while [ ! -f "$dir/release.1.1" ]; do sleep 0.1; done
  fi
  printf '{"type":"assistant","message":{"id":"m%s%s","content":[{"type":"text","text":"reply %s.%s"}]}}\n' "$n" "$m" "$n" "$m"
  printf '{"type":"result","subtype":"success","duration_ms":10}\n'
done
"#;

/// The modeline of the selected window, which speaks for a selected dock.
fn modeline(state: &Value) -> String {
    state["modeline"].as_str().unwrap_or_default().to_owned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_stopped_native_session_resumes_in_its_buffer_and_forks_natively() {
    let flow = Flow::new("native-resume");
    let fake = flow.scratch.path().join("fake-claude");
    let script = RESUME_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");
    // Back to the transcript, whose rows the flow reads.
    app.send_keys("C-c C-o");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), MESSAGE);
    // Its turn ends before the stop, which would ask first while it works.
    end_turn(&flow, &mut app, "release.1.1");
    app.wait_for("the first reply", |state| text(state).contains("reply 1.1"));

    // Stopped from its buffer: idle, so it is hung up at once.
    app.run_command("session-stop", None).unwrap();
    app.wait_for("the session stopped", |state| {
        modeline(state).contains("stopped")
    });

    // Resumed natively: the same buffer, its rows kept, a "Resumed" row, the composer selected.
    app.run_command("session-resume", None).unwrap();
    let args = flow.wait_for_file("run.2");
    assert!(args.contains("arg --resume="), "{args}");
    assert!(args.contains("arg --input-format=stream-json"), "{args}");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("again"));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "again");
    app.send_keys("C-c C-o");
    let state = app.wait_for("the second reply", |state| {
        text(state).contains("reply 2.1")
    });
    assert_eq!(state["buffer"]["name"], "*claude: proj*");
    let shown = text(&state);
    assert!(shown.contains("reply 1.1"), "its rows are kept: {shown}");
    assert!(shown.contains("Resumed."), "{shown}");

    // A fork of it is native too, in a new buffer, its composer selected.
    app.run_command("session-fork", None).unwrap();
    let args = flow.wait_for_file("run.3");
    assert!(args.contains("arg --fork-session"), "{args}");
    assert!(args.contains("arg --input-format=stream-json"), "{args}");
    let state = app.wait_for("the fork's composer", in_composer);
    assert_ne!(state["buffer"]["name"], "*claude: proj*", "a new buffer");
    app.send_keys(&keys("forked"));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.3.1").trim(), "forked");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// Whether a buffer named `name` is open.
fn has_buffer(state: &Value, name: &str) -> bool {
    (state["buffers"].as_array()).is_some_and(|buffers| buffers.iter().any(|b| b["name"] == name))
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_session_stopped_before_its_first_message_is_forgotten() {
    let flow = Flow::new("native-unspoken");
    let fake = flow.scratch.path().join("fake-claude");
    let script = RESUME_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    // `claude` starts it at once, its composer selected; it is stopped before anything is sent.
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    flow.wait_for_file("run.1");
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.run_command("session-stop", None).unwrap();
    let state = app.wait_for("the session forgotten", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("nothing is left to resume"))
    });
    assert!(
        !has_buffer(&state, "*compose: proj*"),
        "{}",
        state["buffers"]
    );
    assert!(
        !has_buffer(&state, "*claude: proj*"),
        "{}",
        state["buffers"]
    );
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// Starts a native session in the flow's trusted project, from `notes.md`, has it answer one
/// message and stops it.
pub(super) fn spoken_and_stopped(flow: &Flow) -> crate::harness::App {
    let fake = flow.scratch.path().join("fake-claude");
    let script = RESUME_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");
    app.send_keys("C-c C-o");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), MESSAGE);
    end_turn(flow, &mut app, "release.1.1");
    app.wait_for("the first reply", |state| text(state).contains("reply 1.1"));
    app.run_command("session-stop", None).unwrap();
    app.wait_for("the session stopped", |state| {
        modeline(state).contains("stopped")
    });
    app
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_session_resumes_with_no_file_of_its_project_open() {
    let flow = Flow::new("native-resume-unopened");
    let mut app = spoken_and_stopped(&flow);
    // The project's only file is killed: no buffer has the project open now.
    app.send_keys(&format!("C-x b {} RET", keys("notes.md")));
    app.wait_for("notes.md", |state| state["buffer"]["name"] == "notes.md");
    app.send_keys("C-x k RET");
    app.wait_for("notes.md killed", |state| !has_buffer(state, "notes.md"));
    app.send_keys(&format!("C-x b {} RET", keys("*claude: proj*")));
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    // Resumed natively all the same: the record of trusted projects holds its root, by path
    // and identity (ADR 0165, part 4a).
    app.run_command("session-resume", None).unwrap();
    let args = flow.wait_for_file("run.2");
    assert!(args.contains("arg --resume="), "{args}");
    assert!(args.contains("arg --input-format=stream-json"), "{args}");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("again"));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "again");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
