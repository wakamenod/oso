//! A native session's composer and queue (ADR 0165, step 2; ADR 0170), with a fake `claude -p`
//! that reads each message the composer sends, replays it, and ends its turn only once the
//! flow says so: a message sent meanwhile waits in oso's queue and goes when the turn ends.
//!
//! Must not depend on: the app's internals; the queue's rules have their own tests.

use std::fs;

use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, in_composer, text};

/// The fake: for each line of its input, `N` from 1, it writes the message's text to
/// `sent.N`, replays it, and once the flow writes `release.N`, replies `reply N` and ends the
/// turn. It exits once its input ends.
const COMPOSE_FAKE: &str = r#"#!/bin/sh
dir='DIR'
cat "$dir/stream.jsonl"
n=0
while IFS= read -r line; do
  case "$line" in *'"control_request"'*) continue ;; esac
  n=$((n+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n"
  printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
  while [ ! -f "$dir/release.$n" ]; do sleep 0.1; done
  printf '{"type":"assistant","message":{"id":"m%s","content":[{"type":"text","text":"reply %s"}]}}\n' "$n" "$n"
  printf '{"type":"result","subtype":"success","duration_ms":10}\n'
done
"#;

/// The rows the transcript shows, read from it, whatever window is selected.
fn transcript(app: &mut super::super::super::harness::App) -> String {
    app.send_keys("C-c C-o");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    text(&state)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_message_sent_while_a_turn_runs_waits_in_the_queue_and_goes_when_it_ends() {
    let flow = Flow::new("native-compose");
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
    // Back to the transcript, whose rows the flow reads.
    app.send_keys("C-c C-o");
    assert_eq!(flow.wait_for_file("sent.1").trim(), MESSAGE);
    flow.scratch.write("release.1", b"");
    app.wait_for("the first reply", |state| text(state).contains("reply 1"));

    // `i` goes to the composer, docked under the transcript; `C-c C-c` sends at once.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("second"));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.2").trim(), "second");

    // While that turn runs, a message waits in the queue, grey at the transcript's end.
    app.send_keys(&keys("third"));
    app.send_keys("C-c C-c");
    let shown = transcript(&mut app);
    assert!(shown.contains("Queued (1)\n  1. third"), "{shown}");
    let modeline = app.state()["modeline"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(modeline.contains("1 queued"), "{modeline}");
    assert!(
        !flow.scratch.path().join("sent.3").exists(),
        "not written while busy"
    );
    // A restart would lose it: it is refused, naming the session and quoting nothing.
    for method in ["check-handover", "export-state"] {
        let line = app.exchange(method, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !line.contains("third"),
            "{method} quotes the message: {line}"
        );
        assert!(line.contains("queued message"), "{method}: {line}");
    }

    // The turn ends: the queued message goes.
    flow.scratch.write("release.2", b"");
    assert_eq!(flow.wait_for_file("sent.3").trim(), "third");
    flow.scratch.write("release.3", b"");
    let state = app.wait_for("the third reply", |state| text(state).contains("reply 3"));
    let shown = text(&state);
    assert!(shown.contains("› third"), "{shown}");
    assert!(!shown.contains("Queued"), "{shown}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
