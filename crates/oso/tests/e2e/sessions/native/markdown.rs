//! A native session's reply drawn as its Markdown says (ADR 0165, part 2g and "The look:
//! Markdown"), with a fake `claude -p` that streams the start of a reply, then, once the flow
//! says so, its whole message: a line that has not ended as written, every other without its
//! marks, its Rust fence highlighted once whole, and on macOS, the one OS that draws text so
//! far, drawn in colour that `global-font-lock-mode` takes away. `w` copies the Markdown, `M-w`
//! what shows.
//!
//! Must not depend on: the app's internals; which runs a reply's row takes, and when, have
//! their own tests.

use std::fs;

use serde_json::{Value, json};

use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, in_composer, text};

/// The fake: the stream the flow wrote, then once the flow writes `whole`, the rest of it,
/// then the exit the flow writes to `go`.
const MARKDOWN_FAKE: &str = r#"#!/bin/sh
dir='DIR'
IFS= read -r first
cat "$dir/stream.jsonl"
while [ ! -f "$dir/whole" ]; do sleep 0.1; done
cat "$dir/after.jsonl"
while [ ! -f "$dir/go" ]; do sleep 0.1; done
exit "$(cat "$dir/go")"
"#;

/// The reply: inline code and a Rust fence.
const REPLY: &str = "Notes:\n\nSome `code` here.\n\n```rust\nfn main() {}\n```";

fn lines(lines: &[Value]) -> String {
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

/// The person's message replayed, and the reply's first line streamed, its block open.
fn streamed() -> String {
    let start = json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}});
    let delta = json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Notes:\n\nSome `co"}});
    lines(&[
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "stream_event", "api_message_id": "m1", "event": start}),
        json!({"type": "stream_event", "api_message_id": "m1", "event": delta}),
    ])
}

/// The reply's whole message and the turn's end.
fn after() -> String {
    lines(&[
        json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "text", "text": REPLY}]}}),
        json!({"type": "result", "subtype": "success", "duration_ms": 10}),
    ])
}

/// Whether the session's buffer holds runs of code's classes: only the parse of the reply's
/// fence can give them here, as the stream has no tool row.
fn parsed(state: &Value) -> bool {
    state["buffer"]["name"] == "*claude: proj*" && state["buffer"]["code"] == true
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_reply_is_drawn_without_its_marks_and_highlighted_once_whole() {
    let flow = Flow::new("native-markdown");
    let fake = flow.scratch.path().join("fake-claude");
    let script = MARKDOWN_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    flow.scratch.write("stream.jsonl", streamed().as_bytes());
    flow.scratch.write("after.jsonl", after().as_bytes());
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
    // Back to the transcript, whose rows and highlights the flow reads.
    app.send_keys("C-c C-o");

    // The line that streams is as written, and plain.
    let state = app.wait_for("the reply streaming", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("Some `co")
    });
    assert!(!parsed(&state), "plain while it streams: {state}");

    // Whole, it is highlighted.
    flow.scratch.write("whole", b"");
    let state = app.wait_for("the reply highlighted", |state| {
        parsed(state) && text(state).contains("fn main() {}")
    });
    let shown = text(&state);
    let drawn = "● Notes:\n\n  Some code here.\n\n  fn main() {}\n";
    assert!(shown.contains(drawn), "{shown}");
    // Only macOS draws text so far (ADR 0036): elsewhere the state alone is checked.
    let shot = cfg!(target_os = "macos").then(|| app.screenshot());

    app.run_command("global-font-lock-mode", None).unwrap();
    app.wait_for("the reply plain again", |state| {
        state["buffer"]["name"] == "*claude: proj*" && state["buffer"]["syntax"] == false
    });
    if let Some(shot) = &shot {
        let plain = app.screenshot();
        // The transcript's rows, above the composer, the modeline and the echo area.
        let rows = 0..shot.height / 3;
        assert!(
            shot.differs_in(&plain, rows) > 0,
            "the reply's colour is drawn, and goes with font-lock"
        );
    }
    app.run_command("global-font-lock-mode", None).unwrap();
    app.wait_for("the reply highlighted again", parsed);

    // `w` copies the reply as Claude wrote it, `M-w` what shows: yanked in the composer.
    let mut copied = |what: &str, keys: &str, copied: &str| {
        app.send_keys("M-< C-s S o m e RET C-a");
        app.send_keys(keys);
        app.send_keys("i");
        app.wait_for("the composer", in_composer);
        app.send_keys("C-y");
        app.wait_for(what, |state| {
            in_composer(state) && text(state).trim_end() == copied
        });
        app.send_keys("C-x h C-w C-c C-o");
        app.wait_for("the transcript", |state| {
            state["buffer"]["name"] == "*claude: proj*"
        });
    };
    copied("the Markdown yanked", "w", REPLY);
    copied("what shows yanked", "C-SPC C-e M-w", "Some code here.");

    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
