//! A native session's to-do strip (ADR 0165, part 3e), with a fake `claude -p` whose stream
//! creates three tasks and starts one, then, once the flow says so, completes them all and
//! ends its turn: the strip shows one line docked under the transcript, `C-c C-t` opens it to
//! a row each and folds it back, and it goes at the turn's end.
//!
//! Must not depend on: the app's internals; the list's rules and the strip's text have their
//! own tests.

use std::fs;

use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, in_composer, text};
use serde_json::{Value, json};

/// The fake: the stream the flow wrote, then once the flow writes `done`, the rest of it, then
/// the exit the flow writes to `go`.
const TODO_FAKE: &str = r#"#!/bin/sh
dir='DIR'
IFS= read -r first
cat "$dir/stream.jsonl"
while [ ! -f "$dir/done" ]; do sleep 0.1; done
cat "$dir/after.jsonl"
while [ ! -f "$dir/go" ]; do sleep 0.1; done
exit "$(cat "$dir/go")"
"#;

const TASKS: [&str; 3] = [
    "Read the handler",
    "Run the auth tests",
    "Update the changelog",
];

fn lines(lines: &[Value]) -> String {
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

/// A call of `name` with `input`, as call `id`, and its result, which gave back `result`.
fn call(id: &str, name: &str, input: &Value, result: &Value) -> [Value; 2] {
    [
        json!({"type": "assistant", "message": {"id": format!("m{id}"), "content": [
            {"type": "tool_use", "id": id, "name": name, "input": input}
        ]}}),
        json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]},
            "tool_use_result": result
        }),
    ]
}

/// Task `n` (from 1) set to `status`, as call `id`.
fn update(id: &str, n: usize, status: &str) -> [Value; 2] {
    let input = json!({"taskId": n.to_string(), "status": status});
    let result = json!({
        "success": true,
        "taskId": n.to_string(),
        "updatedFields": ["status"],
        "statusChange": {"from": "pending", "to": status}
    });
    call(id, "TaskUpdate", &input, &result)
}

/// The person's message replayed, three tasks created and the second started.
fn started() -> String {
    let mut stream = vec![
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
    ];
    for (at, task) in TASKS.iter().enumerate() {
        let input = json!({"subject": task, "description": task});
        let result = json!({"task": {"id": (at + 1).to_string(), "subject": task}});
        stream.extend(call(&format!("c{at}"), "TaskCreate", &input, &result));
    }
    stream.extend(update("u1", 1, "completed"));
    stream.extend(update("u2", 2, "in_progress"));
    lines(&stream)
}

/// The rest done, and the turn's end.
fn finished() -> String {
    let mut stream = Vec::new();
    stream.extend(update("u3", 2, "completed"));
    stream.extend(update("u4", 3, "completed"));
    stream.push(json!({"type": "result", "subtype": "success", "duration_ms": 10}));
    lines(&stream)
}

/// The window docked in the strip's slot, if any.
fn strip(state: &Value) -> Option<Value> {
    (state["windows"].as_array()?.iter())
        .find(|window| window["docked"]["slot"] == "strip")
        .cloned()
}

/// The strip's rows, read with it selected: `C-x o` from the transcript steps to it, as no
/// card is docked above it, and `C-c C-o` from the composer comes back.
fn strip_text(app: &mut App, what: &str) -> String {
    app.send_keys("C-x o");
    let state = app.wait_for(what, |state| state["buffer"]["name"] == "*todo: proj*");
    // To the composer under it, then back to the transcript.
    app.send_keys("C-x o");
    app.wait_for("the composer", in_composer);
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    text(&state)
}

use super::super::super::harness::App;

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_to_do_strip_shows_opens_folds_and_goes_at_the_turns_end() {
    let flow = Flow::new("native-todo");
    let fake = flow.scratch.path().join("fake-claude");
    let script = TODO_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(&fake, script).unwrap();
    flow.scratch.write("stream.jsonl", started().as_bytes());
    flow.scratch.write("after.jsonl", finished().as_bytes());
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

    // One line, docked under the transcript.
    app.wait_for("the strip folded", |state| {
        state["buffer"]["name"] == "*claude: proj*"
            && strip(state).is_some_and(|strip| strip["rows"] == 1)
    });
    let folded = strip_text(&mut app, "the strip selected");
    assert!(folded.starts_with("☐ 1/3 ▶ Run the auth tests"), "{folded}");
    assert!(folded.trim_end().ends_with("C-c C-t list"), "{folded}");

    // `C-c C-t` opens it to a row each, and folds it back.
    app.send_keys("C-c C-t");
    app.wait_for("the strip opened", |state| {
        strip(state).is_some_and(|strip| strip["rows"] == 3)
    });
    let opened = strip_text(&mut app, "the strip opened, selected");
    assert_eq!(
        opened.lines().map(str::trim_end).collect::<Vec<_>>(),
        [
            "☑ Read the handler",
            "▶ Run the auth tests",
            "☐ Update the changelog"
        ],
    );
    app.send_keys("C-c C-t");
    app.wait_for("the strip folded again", |state| {
        strip(state).is_some_and(|strip| strip["rows"] == 1)
    });

    // All done and the turn ended: it goes.
    flow.scratch.write("done", b"");
    app.wait_for("the strip gone", |state| strip(state).is_none());

    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
