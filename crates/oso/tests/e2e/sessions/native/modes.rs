//! A native session's permission mode (ADR 0165, part 3b), with a fake `claude -p` that answers
//! `set_permission_mode` or not, as the flow says: on tier 2 `S-TAB` is the request, the
//! process stays, and the footer changes only once the CLI says the new mode; on tier 1 the
//! idle process ends and resumes with `--permission-mode`, a switch during a turn is refused,
//! and an interrupt's resume keeps the mode.
//!
//! Must not depend on: the app's internals; the mode's rules have their own tests.

use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, in_composer, text};

/// The fake: run `N` from 1 writes its arguments to `run.N`, and starts in the mode its
/// `--permission-mode` gives, else `default`. It answers `initialize` with that mode when
/// `tier` says 2, and refuses it otherwise. A `set_permission_mode` it notes in `asked.N`,
/// answers at once, and says the new mode in a `system/status` only once `say.MODE` is there.
/// Each message `M` it writes to `sent.N.M`, starts a turn whose `system/init` says the mode,
/// and, unless it starts with `long`, answers `reply N.M` and ends the turn. `SIGINT` ends the turn and the process, as the CLI's does,
/// noting `signalled.N`.
pub(super) const MODES_FAKE: &str = r#"#!/bin/sh
dir='DIR'
n=$(ls "$dir" | grep -c '^run\.')
n=$((n+1))
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$n"
mv "$dir/tmp.$n" "$dir/run.$n"
tier=$(cat "$dir/tier")
mode=default
for a in "$@"; do
  case "$a" in --permission-mode=*) mode=${a#--permission-mode=} ;; esac
done
aborted='{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming"}'
trap 'printf "%s\n" "$aborted"; echo x > "$dir/signalled.$n"; exit 0' INT
m=0
while IFS= read -r line; do
  case "$line" in
    *'"control_request"'*)
      id=$(printf '%s\n' "$line" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
      case "$line" in
        *'"initialize"'*)
          if [ "$tier" = 2 ]; then
            printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"current_permission_mode":"%s"}}}\n' "$id" "$mode"
          else
            printf '{"type":"control_response","response":{"subtype":"error","request_id":"%s","error":"unsupported"}}\n' "$id"
          fi ;;
        *'"set_permission_mode"'*)
          asked=$(printf '%s\n' "$line" | sed 's/.*"mode":"\([^"]*\)".*/\1/')
          printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"mode":"%s"}}}\n' "$id" "$asked"
          printf '%s\n' "$asked" > "$dir/asked.$n"
          while [ ! -f "$dir/say.$asked" ]; do sleep 0.05; done
          mode=$asked
          printf '{"type":"system","subtype":"status","status":null,"permissionMode":"%s"}\n' "$mode" ;;
      esac
      continue ;;
  esac
  m=$((m+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$n.$m"
  printf '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","permissionMode":"%s","capabilities":[]}\n' "$mode"
  printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
  case "$said" in
    long*) ;;
    *)
      printf '{"type":"assistant","message":{"id":"m%s%s","content":[{"type":"text","text":"reply %s.%s"}]}}\n' "$n" "$m" "$n" "$m"
      printf '{"type":"result","subtype":"success","duration_ms":10}\n' ;;
  esac
done
"#;

/// A flow whose fake is [`MODES_FAKE`] on `tier`, started, with the composer selected.
fn started(test: &str, tier: u8) -> (Flow, App) {
    let flow = Flow::with(test, MODES_FAKE, "");
    flow.scratch.write("tier", tier.to_string().as_bytes());
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    (flow, app)
}

pub(super) fn modeline(state: &Value) -> &str {
    state["modeline"].as_str().unwrap_or_default()
}

/// What the session's footer says.
pub(super) fn footer(state: &Value) -> &str {
    state["footer"].as_str().unwrap_or_default()
}

/// The state once the footer says `words`, from the composer or the transcript; the modeline beside
/// it names no mode.
pub(super) fn footer_says(app: &mut App, words: &str) -> Value {
    let state = app.wait_for(words, |state| footer(state).contains(words));
    for mode in ["default", "acceptEdits", "plan", "auto"] {
        assert!(!modeline(&state).contains(mode), "{state}");
    }
    state
}

/// Sends `message` from the composer.
pub(super) fn send(app: &mut App, message: &str) {
    app.send_keys(&keys(message));
    app.send_keys("C-c C-c");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn on_tier_two_s_tab_asks_and_the_footer_waits_for_the_clis_word() {
    let (flow, mut app) = started("native-modes-2", 2);
    // `initialize` says the mode before the first turn.
    footer_says(&mut app, "⏸ manual mode on (S-TAB to cycle)");

    // From the composer: the request goes, and the mode stays until the CLI says so.
    app.send_keys("S-TAB");
    assert_eq!(flow.wait_for_file("asked.1").trim(), "acceptEdits");
    thread::sleep(Duration::from_millis(300));
    footer_says(&mut app, "⏸ manual mode on (changing…)");
    flow.scratch.write("say.acceptEdits", b"x");
    footer_says(&mut app, "▶▶ accept edits on (S-TAB to cycle)");

    // From the transcript, the next.
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.send_keys("S-TAB");
    flow.scratch.write("say.plan", b"x");
    footer_says(&mut app, "⏸ plan mode on");
    assert_eq!(flow.wait_for_file("asked.1").trim(), "plan");
    // Past the request's wait, nothing fell back to a resume.
    thread::sleep(Duration::from_millis(2_500));
    assert!(
        !flow.scratch.path().join("run.2").exists(),
        "the process stays on tier 2"
    );
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn on_tier_one_s_tab_resumes_an_idle_session_in_the_mode_and_an_interrupt_keeps_it() {
    let (flow, mut app) = started("native-modes-1", 1);
    send(&mut app, "one");
    assert_eq!(flow.wait_for_file("sent.1.1").trim(), "one");
    footer_says(&mut app, "⏸ manual mode on");

    // Idle: claude ends and resumes in the next mode, which its next turn says.
    app.send_keys("S-TAB");
    let args = flow.wait_for_file("run.2");
    assert!(args.contains("arg --resume="), "{args}");
    assert!(args.contains("arg --permission-mode=acceptEdits"), "{args}");
    // The process that runs now said none yet.
    footer_says(&mut app, "mode not known");
    app.send_keys("C-c C-o");
    app.wait_for("the row", |state| {
        state["buffer"]["name"] == "*claude: proj*"
            && text(state).contains("Changing the mode to acceptEdits")
    });
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    send(&mut app, "two");
    assert_eq!(flow.wait_for_file("sent.2.1").trim(), "two");
    footer_says(&mut app, "▶▶ accept edits on");

    // During a turn, refused.
    send(&mut app, "long three");
    assert_eq!(flow.wait_for_file("sent.2.2").trim(), "long three");
    app.send_keys("S-TAB");
    app.wait_for("the refusal", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("it can change once idle"))
    });
    assert!(!flow.scratch.path().join("run.3").exists());

    // An interrupt's resume keeps the mode, which `--resume` alone would not.
    app.send_keys("C-c C-k");
    flow.wait_for_file("signalled.2");
    let args = flow.wait_for_file("run.3");
    assert!(args.contains("arg --permission-mode=acceptEdits"), "{args}");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
