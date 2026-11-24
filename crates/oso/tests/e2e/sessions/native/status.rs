//! A native session's status line (ADR 0165, part 3a), with a fake `claude -p` that answers
//! `initialize` with its models and `get_usage` with a cost and two windows: before its first
//! turn the transcript's modeline says the cost and the more used window, after it the context
//! used, the cost and the event's window, and the footer the model that answered and the mode; a
//! narrow window drops the modeline's from the right. Each line is read with `*Spaces*` hidden, the transcript's window
//! the surface's 100 columns: beside the list it is too narrow for the line.
//!
//! Must not depend on: the app's internals; the line's rules have their own tests.

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, in_composer};

/// The fake: it answers `initialize` with two models and `get_usage`, by the id it was asked
/// with, with a cost and two windows. Each message `M` it writes to `sent.M` and answers as
/// Haiku, though it started as Sonnet, with 10 000 tokens read of a 200 000 window, a week's
/// limit 8% used, and a mode change after the turn's end. The values are short, for the whole
/// line to fit in 100 columns.
const STATUS_FAKE: &str = r#"#!/bin/sh
dir='DIR'
m=0
while IFS= read -r line; do
  case "$line" in
    *'"control_request"'*)
      id=$(printf '%s\n' "$line" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
      case "$line" in
        *'"initialize"'*)
          printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"oso-0","response":{"models":[{"value":"haiku","resolvedModel":"claude-haiku-5-5","displayName":"Haiku 5.5"},{"value":"sonnet","resolvedModel":"claude-sonnet-5-5","displayName":"Sonnet 5.5"}]}}}' ;;
        *'"get_usage"'*)
          printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"rate_limits":{"five_hour":{"utilization":63},"seven_day":{"utilization":28}},"session":{"total_cost_usd":0.5}}}}\n' "$id"
          printf 'asked\n' > "$dir/usage.asked" ;;
      esac
      continue ;;
  esac
  m=$((m+1))
  printf '%s\n' "$line" > "$dir/sent.$m"
  printf '%s\n' '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"claude-sonnet-5-5","permissionMode":"default"}'
  printf '%s\n' '{"type":"assistant","message":{"id":"m1","model":"claude-haiku-5-5","content":[{"type":"text","text":"Planned."}],"usage":{"input_tokens":10,"cache_creation_input_tokens":990,"cache_read_input_tokens":9000,"output_tokens":7}}}'
  printf '%s\n' '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"seven_day","unifiedWindows":{"seven_day":{"utilization":0.08}}}}'
  printf '%s\n' '{"type":"result","subtype":"success","duration_ms":10,"total_cost_usd":1.2,"modelUsage":{"claude-haiku-5-5":{"contextWindow":200000},"claude-sonnet-5-5":{"contextWindow":1000000}}}'
  printf '%s\n' '{"type":"system","subtype":"status","status":null,"permissionMode":"plan"}'
done
"#;

/// What the line says beside the footer once the turn ended.
const AFTER: &str = "ctx 5% · $1.20 · 7d 8%";

/// The modeline's text, or nothing.
fn modeline(state: &Value) -> &str {
    state["modeline"].as_str().unwrap_or_default()
}

/// Hides `*Spaces*`, which shows on its own beside a session, with `C-u C-c a s` (ADR 0140);
/// the composer stays selected.
fn hide_the_side_list(app: &mut App) {
    app.wait_for("the list shown", |state| {
        !state["sessions"]["side"].is_null()
    });
    app.send_keys("C-u C-c a s");
    app.wait_for("the list hidden", |state| {
        state["sessions"]["side"].is_null()
    });
    app.wait_for("the composer still selected", in_composer);
}

/// The transcript's modeline once it says `what`, its point at its start.
fn transcript_says(app: &mut App, what: &str) -> Value {
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.send_keys("M-<");
    app.wait_for(what, |state| {
        state["buffer"]["name"] == "*claude: proj*" && modeline(state).contains(what)
    })
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_transcripts_modeline_says_the_sessions_model_mode_context_cost_and_limit() {
    let flow = Flow::with("native-status", STATUS_FAKE, "");
    // The instance's state is kept in the project, which the turn's look at its changes would
    // find, making the session ready for review and its attention mark crowd the line.
    let exclude = flow.project.join(".git/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "state/\n").unwrap();
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    hide_the_side_list(&mut app);

    // Before the first turn: get_usage's cost and the more used of its windows.
    flow.wait_for_file("usage.asked");
    transcript_says(&mut app, "$0.50 · 5h 63%");
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("plan it"));
    app.send_keys("C-c C-c");
    flow.wait_for_file("sent.1");
    // The turn ended while the composer was selected: until its transcript has been in front
    // of the person for a moment (ADR 0140), the session's attention mark leads the modeline
    // and crowds the line, and a CI window that lost its focus never has the person in front.
    app.call("focus", Some(json!({ "focused": true })));
    let state = transcript_says(&mut app, AFTER);
    assert!(modeline(&state).ends_with(AFTER), "{}", modeline(&state));
    // The footer says the model and the mode, and the modeline neither.
    let footer = state["footer"].as_str().unwrap_or_default();
    assert_eq!(footer, "⏸ plan mode on (S-TAB to cycle)  Haiku 5.5");
    for said in ["Haiku", "plan ·"] {
        assert!(!modeline(&state).contains(said), "{}", modeline(&state));
    }
    narrow_windows_drop_parts_from_the_right(&mut app);
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// Splits the transcript's window until it is too narrow for the whole line: the parts go
/// from the right, the context's first stays.
fn narrow_windows_drop_parts_from_the_right(app: &mut App) {
    let wide = app.state();
    let cells = modeline(&wide).chars().count();
    for _ in 0..4 {
        if app.state()["window"]["columns"].as_u64().unwrap_or(0) < cells as u64 {
            break;
        }
        app.send_keys("C-x 3");
    }
    let state = app.wait_for("the window's split", |state| {
        state["window"]["columns"].as_u64().unwrap_or(0) < cells as u64
    });
    let narrow = modeline(&state);
    assert!(narrow.contains("ctx 5%"), "{narrow}");
    assert!(!narrow.contains("7d 8%"), "{narrow}");
    let shown = AFTER.split(" · ").filter(|part| narrow.contains(part));
    let kept: Vec<&str> = shown.collect();
    assert!(AFTER.starts_with(&kept.join(" · ")), "{narrow}");
}
