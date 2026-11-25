//! A message being sent (ADR 0165), with a fake `claude -p` that replays what it reads only
//! when the flow says so: shown at once at the transcript's end, a prompt once it is replayed,
//! said not to be confirmed when its turn ends without, and back in the composer when the
//! process ends first.
//!
//! Must not depend on: the app's internals; what stands where has its own tests.

use std::fs;

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, in_composer, joined};

/// The fake: for each message of its input, `N` from 1, it writes `sent.N`, replays it once the
/// flow writes `replay.N`, and ends its turn once it writes `release.N`; with `die.N` it exits
/// at once, with status 3.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
cat "$dir/stream.jsonl"
n=0
while IFS= read -r line; do
  case "$line" in *'"control_request"'*) continue ;; esac
  n=$((n+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n"
  replayed=
  while [ ! -f "$dir/release.$n" ]; do
    [ -f "$dir/die.$n" ] && exit 3
    if [ -z "$replayed" ] && [ -f "$dir/replay.$n" ]; then
      replayed=1
      printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
    fi
    sleep 0.1
  done
  printf '{"type":"assistant","message":{"id":"m%s","content":[{"type":"text","text":"reply %s"}]}}\n' "$n" "$n"
  printf '{"type":"result","subtype":"success","duration_ms":10}\n'
done
"#;

/// Writes `message` in the composer, the `n`th, and sends it.
fn send(app: &mut App, flow: &Flow, message: &str, n: usize) {
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(message));
    app.send_keys("C-c C-c");
    app.send_keys("C-c C-o");
    assert_eq!(flow.wait_for_file(&format!("sent.{n}")).trim(), message);
}

/// The transcript's rows, once they hold `wanted`.
fn shows(app: &mut App, wanted: &str) -> String {
    let state = app.wait_for(wanted, |state| {
        state["buffer"]["name"] == "*claude: proj*" && joined(state).contains(wanted)
    });
    joined(&state)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_message_shows_at_once_and_says_what_became_of_it() {
    let flow = Flow::new("native-sending");
    let fake = flow.scratch.path().join("fake-claude");
    let script = FAKE.replace("DIR", &flow.scratch.path().display().to_string());
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

    // At once, with nothing of the CLI's: not the person's prompt yet.
    let shown = shows(&mut app, &format!("…  {MESSAGE}"));
    assert!(!shown.contains('〉'), "{shown}");
    assert_eq!(flow.wait_for_file("sent.1").trim(), MESSAGE);
    flow.scratch.write("replay.1", b"");
    let shown = shows(&mut app, &format!("〉 {MESSAGE}"));
    assert!(!shown.contains('…'), "{shown}");
    flow.scratch.write("release.1", b"");
    shows(&mut app, "reply 1");

    // Its turn ends with no replay; a later message is confirmed, and passes it over.
    send(&mut app, &flow, "second", 2);
    shows(&mut app, "…  second");
    flow.scratch.write("release.2", b"");
    shows(&mut app, "✗  second");
    shows(&mut app, "claude did not say it took this");
    send(&mut app, &flow, "third", 3);
    let shown = shows(&mut app, "…  third");
    assert!(shown.contains("✗  second"), "not its turn: {shown}");
    flow.scratch.write("replay.3", b"");
    let shown = shows(&mut app, "〉 third");
    let passed = shown.find("Not confirmed by claude: second");
    assert!(
        passed < shown.find("〉 third") && passed.is_some(),
        "{shown}"
    );
    flow.scratch.write("release.3", b"");
    shows(&mut app, "reply 3");

    // The process ends first: it is said, and the text is in the composer again.
    send(&mut app, &flow, "fourth", 4);
    shows(&mut app, "…  fourth");
    flow.scratch.write("die.4", b"");
    shows(&mut app, "and back in the composer: fourth");
    app.send_keys("i");
    app.wait_for("the text in the composer", |state| {
        in_composer(state) && joined(state).trim() == "fourth"
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
