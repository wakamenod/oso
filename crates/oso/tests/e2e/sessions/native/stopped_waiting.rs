//! A native session's card whose call the CLI stops waiting for (ADR 0165, part 2; ADR 0182,
//! "Gone otherwise"): with a fake `claude -p` that asks through `oso mcp --permission` about an
//! `AskUserQuestion` its stream shows, then cancels the call, or gives it a result in the
//! stream, as Claude Code does when it gives up on a tool, before the person answers. The card
//! closes, and the call's row says why, so that no answer is lost unseen.
//!
//! Must not depend on: the app's internals; the queue and the bridge have their own tests.

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::cards::{ask, questions, stream};
use super::{Flow, card, in_composer, joined, sent, text};

/// The fake: as the cards' fake, but its bridge, started in the background, asks about the
/// first call `asks.jsonl` holds and sends `cancel.jsonl` once the flow writes `cancel`, its
/// lines going to `bridge.out`; and the fake prints `result.jsonl` on its stream once the flow
/// writes `result`.
const STOPPING_FAKE: &str = r#"#!/bin/sh
dir='DIR'
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
while IFS= read -r first; do
  case "$first" in *'"control_request"'*) ;; *) break ;; esac
done
cat "$dir/stream.jsonl"
while [ ! -f "$dir/ask" ]; do sleep 0.1; done
oso=$(sed -n 's/.*"command":"\([^"]*\)".*/\1/p' "$dir/run.$$" | head -n 1)
pid=$(sed -n 's/.*"--pid","\([0-9]*\)".*/\1/p' "$dir/run.$$" | head -n 1)
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
  head -n 1 "$dir/asks.jsonl"
  while [ ! -f "$dir/cancel" ] && [ ! -f "$dir/go" ]; do sleep 0.1; done
  [ -f "$dir/cancel" ] && cat "$dir/cancel.jsonl"
  while [ ! -f "$dir/go" ]; do sleep 0.1; done
} | "$oso" mcp --pid "$pid" --permission > "$dir/bridge.out" &
while [ ! -f "$dir/result" ] && [ ! -f "$dir/go" ]; do sleep 0.1; done
[ -f "$dir/result" ] && cat "$dir/result.jsonl"
while [ ! -f "$dir/go" ]; do sleep 0.1; done
exit "$(cat "$dir/go")"
"#;

/// What the call's row says once the CLI stopped waiting for the person's answer.
const STOPPED: &str = "denied: Claude Code stopped waiting before you answered";

/// A flow whose fake asks about the questions' call `q1` once the flow writes `ask`.
fn flow(test: &str) -> Flow {
    let flow = Flow::with(test, STOPPING_FAKE, &stream());
    let asks = ask(2, "AskUserQuestion", "q1", &questions());
    flow.scratch
        .write("asks.jsonl", format!("{asks}\n").as_bytes());
    flow
}

/// The app, once the questions' card is shown.
fn card_shown(flow: &Flow) -> App {
    let mut app = sent(flow);
    app.wait_for("the calls", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("AskUserQuestion")
    });
    flow.scratch.write("ask", b"");
    let shown = card(&mut app, "the question card");
    assert!(shown.starts_with("╭─ Claude asks 1/2 "), "{shown}");
    app
}

/// Waits until the card is gone and point is in the composer, then reads the transcript: the
/// questions' call's row says [`STOPPED`].
fn closed_with_its_row(app: &mut App) {
    let state = app.wait_for("the card closed", |state| {
        in_composer(state)
            && !(state["windows"].as_array())
                .is_some_and(|windows| windows.iter().any(|w| w["docked"]["slot"] == "card"))
    });
    assert!(
        (state["echo"].as_str()).is_some_and(|echo| echo.contains("Claude Code stopped waiting")),
        "{}",
        state["echo"]
    );
    app.send_keys("C-c C-o");
    // Read joined, as the row wraps in the window.
    app.wait_for("the call's row", |state| {
        state["buffer"]["name"] == "*claude: proj*"
            && joined(state).contains(&format!("AskUserQuestion · Which size? · {STOPPED}"))
    });
}

/// The lines the bridge wrote to the fake, once there are `n`.
fn bridge_lines(flow: &Flow, n: usize) -> Vec<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let out =
            std::fs::read_to_string(flow.scratch.path().join("bridge.out")).unwrap_or_default();
        let lines: Vec<Value> = (out.lines())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if lines.len() >= n {
            return lines;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the bridge wrote {out:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn end(flow: &Flow, mut app: App) {
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_card_the_cli_cancels_closes_and_its_row_says_so() {
    let flow = flow("native-card-cancelled");
    let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 2, "reason": "MCP tool idle timeout"}});
    flow.scratch
        .write("cancel.jsonl", format!("{cancel}\n").as_bytes());
    let mut app = card_shown(&flow);
    flow.scratch.write("cancel", b"");
    closed_with_its_row(&mut app);
    // The cancelled call is answered nothing: the bridge wrote only `initialize`'s.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let lines = bridge_lines(&flow, 1);
    assert_eq!(lines.len(), 1, "{lines:?}");
    end(&flow, app);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_card_whose_call_got_its_result_closes_and_its_row_says_so() {
    let flow = flow("native-card-result-first");
    let result = json!({"type": "user", "message": {"content": [{
        "type": "tool_result", "tool_use_id": "q1", "is_error": true,
        "content": "MCP server \"oso\" tool \"permission\" sent no response or progress for 1800s; aborting.",
    }]}});
    flow.scratch
        .write("result.jsonl", format!("{result}\n").as_bytes());
    let mut app = card_shown(&flow);
    flow.scratch.write("result", b"");
    closed_with_its_row(&mut app);
    // The bridge, still waiting, is told no: nothing runs that the person did not allow.
    let lines = bridge_lines(&flow, 2);
    let text = lines[1]["result"]["content"][0]["text"].as_str().unwrap();
    let answer: Value = serde_json::from_str(text).unwrap();
    assert_eq!(answer["behavior"], "deny", "{answer}");
    end(&flow, app);
}
