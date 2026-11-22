//! A native session's slash commands (ADR 0165, step 2d), with a fake `claude -p` that lists
//! its commands in `initialize`'s answer: `/` as the composer's first character completes over
//! them, a command sent shows as the person's row though the CLI replays none, and `/clear` is
//! refused.
//!
//! Must not depend on: the app's internals; the commands' rules have their own tests.

use std::thread;
use std::time::{Duration, Instant};

use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, in_composer, text};

/// The fake: it answers `initialize` with three commands, one only a terminal runs. Each
/// message `M` it writes to `sent.M`; a `/`-command it answers as the CLI does, with no replay
/// and a synthetic reply, any other message it replays and answers `reply M`.
const SLASH_FAKE: &str = r#"#!/bin/sh
dir='DIR'
m=0
while IFS= read -r line; do
  case "$line" in
    *'"control_request"'*)
      case "$line" in
        *'"initialize"'*)
          printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"oso-0","response":{"commands":[{"name":"model","description":"Set the AI model","argumentHint":"<model>","builtin":true},{"name":"clear","description":"Start a new session","aliases":["reset","new"],"builtin":true},{"name":"doctor","description":"Check the setup","builtin":true}]}}}' ;;
      esac
      continue ;;
  esac
  m=$((m+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '%s\n' "$said" > "$dir/sent.$m"
  printf '%s\n' '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m","terminal_slash_commands":["doctor"]}'
  case "$said" in
    /*)
      printf '{"type":"assistant","message":{"id":"m%s","model":"<synthetic>","content":[{"type":"text","text":"Set model to %s"}]}}\n' "$m" "$m" ;;
    *)
      printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
      printf '{"type":"assistant","message":{"id":"m%s","content":[{"type":"text","text":"reply %s"}]}}\n' "$m" "$m" ;;
  esac
  printf '{"type":"result","subtype":"success","duration_ms":10}\n'
done
"#;

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_slash_completes_over_the_sessions_commands_and_clear_is_refused() {
    let flow = Flow::with("native-slash", SLASH_FAKE, "");
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

    // `/` opens the commands' prompt once the session listed them; till then it is a `/`.
    let deadline = Instant::now() + Duration::from_secs(20);
    let state = loop {
        app.send_keys("/");
        thread::sleep(Duration::from_millis(200));
        let state = app.state();
        if state["prompts"]
            .as_array()
            .is_some_and(|open| !open.is_empty())
        {
            break state;
        }
        assert!(Instant::now() < deadline, "no commands' prompt");
        app.send_keys("DEL");
    };
    assert_eq!(state["prompts"][0]["prompt"], "Command: ");
    // `/clear` and a terminal's own are not offered, even before the first turn.
    assert_eq!(
        state["prompts"][0]["completion"]["rows"],
        serde_json::json!(["model"]),
        "{state}"
    );
    app.send_keys(&keys("mod"));
    app.send_keys("RET");
    app.wait_for("the command in the composer", |state| {
        in_composer(state) && text(state) == "/model "
    });
    app.send_keys(&keys("haiku"));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.1").trim(), "/model haiku");
    // The person's row, though the CLI replays none, and its reply.
    app.send_keys("C-c C-o");
    let state = app.wait_for("the command's reply", |state| {
        text(state).contains("Set model to")
    });
    assert!(text(&state).contains("› /model haiku"), "{}", text(&state));

    // `/clear` is refused, and stays in the composer.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    // `C-g` leaves the `/` typed.
    app.send_keys("/ C-g");
    app.send_keys(&keys("clear"));
    app.send_keys("C-c C-c");
    let state = app.wait_for("the refusal", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("/clear would start"))
    });
    assert!(in_composer(&state));
    // What goes next is the next message, so `/clear` never went.
    app.send_keys("C-a C-k");
    app.send_keys(&keys("hello"));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file("sent.2").trim(), "hello");

    // `C-c /` puts a command first in what is written.
    app.wait_for("the reply", |state| {
        in_composer(state) && text(state).is_empty()
    });
    app.send_keys(&keys("why"));
    app.send_keys("C-c /");
    app.wait_for("the commands' prompt", |state| {
        state["prompts"]
            .as_array()
            .is_some_and(|open| !open.is_empty())
    });
    app.send_keys(&keys("model"));
    app.send_keys("RET");
    app.wait_for("the command before the text", |state| {
        in_composer(state) && text(state) == "/model why"
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
