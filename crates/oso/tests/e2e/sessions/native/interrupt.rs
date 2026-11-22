//! A native session's interrupt (ADR 0165, step 2c), with a fake `claude -p` that answers the
//! control layer's requests or not, as the flow says: on tier 2 `C-c C-k` and `C-u C-c C-c` are
//! the `interrupt` request and the process stays; on tier 1 the process is interrupted with
//! `SIGINT`, ends, and is resumed with `--resume`.
//!
//! Must not depend on: the app's internals; the interrupt's rules and the queue's have their
//! own tests.

use std::thread;
use std::time::Duration;

use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, in_composer, text};

/// The fake: run `N` from 1 writes its arguments to `run.N`. It answers `initialize` only when
/// `tier` says 2, and each `interrupt` with a success and the turn's aborted end, noting
/// `interrupted.N.M`. Each message `M` it writes to `sent.N.M`, replays and, unless it starts
/// with `long`, answers `reply N.M` and ends the turn. `SIGINT` ends the turn and the process,
/// as the CLI's does, noting `signalled.N`. Every line it reads goes to `lines.N`.
///
/// Only the paths a person meets are flows here: a refused `initialize`, a missing receipt, a
/// request unanswered and a stop after the signal are the rules' own tests.
const INTERRUPT_FAKE: &str = r#"#!/bin/sh
dir='DIR'
n=$(ls "$dir" | grep -c '^run\.')
n=$((n+1))
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/run.$n"
tier=$(cat "$dir/tier")
aborted='{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming"}'
trap 'printf "%s\n" "$aborted"; echo x > "$dir/signalled.$n"; exit 0' INT
answer() {
  id=$(printf '%s\n' "$1" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
  printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id"
}
m=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/lines.$n"
  case "$line" in
    *'"control_request"'*)
      case "$line" in
        *'"initialize"'*) if [ "$tier" = 2 ]; then answer "$line"; fi ;;
        *'"interrupt"'*)
          answer "$line"
          printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}'
          printf '%s\n' "$aborted"
          echo x > "$dir/interrupted.$n.$m" ;;
      esac
      continue ;;
  esac
  m=$((m+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n.$m"
  printf '%s\n' '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m","capabilities":["interrupt_receipt_v1"]}'
  printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
  case "$said" in
    long*) ;;
    *)
      printf '{"type":"assistant","message":{"id":"m%s%s","content":[{"type":"text","text":"reply %s.%s"}]}}\n' "$n" "$m" "$n" "$m"
      printf '{"type":"result","subtype":"success","duration_ms":10}\n' ;;
  esac
done
"#;

/// A flow whose fake is [`INTERRUPT_FAKE`] on `tier`, started, with the composer selected.
fn started(test: &str, tier: u8) -> (Flow, super::super::super::harness::App) {
    let flow = Flow::with(test, INTERRUPT_FAKE, "");
    flow.scratch.write("tier", tier.to_string().as_bytes());
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
    (flow, app)
}

/// Sends `message` from the composer with `send`.
fn send(app: &mut super::super::super::harness::App, message: &str, send: &str) {
    app.send_keys(&keys(message));
    app.send_keys(send);
}

/// The rows the transcript shows once they hold `shown`, read from it, then back to the
/// composer.
fn transcript(app: &mut super::super::super::harness::App, shown: &str) -> String {
    app.send_keys("C-c C-o");
    let state = app.wait_for(shown, |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains(shown)
    });
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    text(&state)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn on_tier_two_an_interrupt_keeps_the_process_and_a_send_now_goes_ahead_of_the_queue() {
    let (flow, mut app) = started("native-interrupt-2", 2);
    send(&mut app, "long one", "C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), "long one");
    // The control layer's `initialize` went first.
    let lines = flow.wait_for_file("lines.1");
    assert!(
        lines
            .lines()
            .next()
            .is_some_and(|l| l.contains("\"initialize\"")),
        "{lines}"
    );
    send(&mut app, "queued two", "C-c C-c");
    transcript(&mut app, "Queued (1)");

    // `C-u C-c C-c`: the turn is interrupted, the message goes first, and the queue goes on.
    send(&mut app, "now three", "C-u C-c C-c");
    flow.wait_for_file("interrupted.1.1");
    assert_eq!(flow.wait_for_file("sent.1.2").trim(), "now three");
    assert_eq!(flow.wait_for_file("sent.1.3").trim(), "queued two");
    let shown = transcript(&mut app, "reply 1.3");
    assert!(shown.contains("✗ interrupted"), "{shown}");
    assert!(!shown.contains("Queued"), "{shown}");

    // `C-c C-k`: the turn is interrupted, and the queue waits for the person.
    send(&mut app, "long four", "C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1.4").trim(), "long four");
    send(&mut app, "five", "C-c C-c");
    app.send_keys("C-c C-k");
    flow.wait_for_file("interrupted.1.4");
    // Once oso saw the turn end, what is queued still waits.
    // The transcript's modeline says so, not the composer's.
    app.send_keys("C-c C-o");
    app.wait_for("the interrupted turn ended", |state| {
        let modeline = state["modeline"].as_str().unwrap_or_default();
        state["buffer"]["name"] == "*claude: proj*"
            && modeline.contains("1 queued")
            && !modeline.contains("interrupting")
    });
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    thread::sleep(Duration::from_millis(300));
    assert!(
        !flow.scratch.path().join("sent.1.5").exists(),
        "the queue waits after an interrupt"
    );
    transcript(&mut app, "Queued (1)");
    // An empty send lets it go.
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1.5").trim(), "five");
    // Past the wait for the interrupted turn's end, nothing fell back to a signal.
    thread::sleep(Duration::from_millis(5_500));
    assert!(
        !flow.scratch.path().join("run.2").exists(),
        "the process stays on tier 2"
    );
    assert!(!flow.scratch.path().join("signalled.1").exists());
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn on_tier_one_an_interrupt_signals_the_process_and_resumes_it() {
    let (flow, mut app) = started("native-interrupt-1", 1);
    send(&mut app, "long one", "C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), "long one");

    // No `initialize` answer: the interrupt is `SIGINT`, and the session resumes.
    app.send_keys("C-c C-k");
    flow.wait_for_file("signalled.1");
    let args = flow.wait_for_file("run.2");
    assert!(args.contains("arg --resume="), "{args}");
    assert!(in_composer(&app.state()), "the composer stays selected");
    let shown = transcript(&mut app, "Resumed.");
    assert!(shown.contains("Interrupted with a signal"), "{shown}");
    assert!(!shown.contains("exited with status"), "{shown}");

    // It takes messages again.
    send(&mut app, "after", "C-c C-c");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "after");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
