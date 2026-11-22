//! A native session's question and plan cards (ADR 0165, step 2e-2), with a fake `claude -p`
//! that asks through `oso mcp --permission` about an `AskUserQuestion` and an `ExitPlanMode`
//! its stream shows: the questions are answered one by one, their answers going back in the
//! call's input, and the plan is denied with a note.
//!
//! Must not depend on: the app's internals; the card's rules have their own tests.

use std::fs;

use serde_json::{Value, json};

use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, answer_after_quiet, card, in_composer, sent, text, wait_for_answers};

/// The fake: as the asking fake, but asks about the two calls `asks.jsonl` holds, the second
/// once the flow writes `ask2`, and writes
/// the bridge's answers to `answers`.
const CARDS_FAKE: &str = r#"#!/bin/sh
dir='DIR'
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/run.$$"
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
  while [ ! -f "$dir/ask2" ]; do sleep 0.1; done
  tail -n +2 "$dir/asks.jsonl"
  while [ ! -f "$dir/answers" ]; do sleep 0.1; done
} | "$oso" mcp --pid "$pid" --permission | {
  IFS= read -r init
  : > "$dir/partial"
  for n in 2 3; do IFS= read -r line; printf '%s\n' "$line" >> "$dir/partial"; done
  mv "$dir/partial" "$dir/answers"
}
while [ ! -f "$dir/go" ]; do sleep 0.1; done
exit "$(cat "$dir/go")"
"#;

/// The questions the fake's `AskUserQuestion` asks.
fn questions() -> Value {
    json!({"questions": [
        {
            "question": "Which size?",
            "header": "Size",
            "multiSelect": false,
            "options": [
                {"label": "Small", "description": "Small size"},
                {"label": "Large", "description": "Large size"},
            ],
        },
        {
            "question": "Which colors?",
            "header": "Colors",
            "multiSelect": true,
            "options": [{"label": "Red"}, {"label": "Green"}, {"label": "Blue"}],
        },
    ]})
}

/// The stream: the message replayed, and the two calls, the plan's with no input, as the CLI
/// writes it (measured).
fn stream() -> String {
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": questions()},
            {"type": "tool_use", "id": "p1", "name": "ExitPlanMode", "input": {}},
        ]}}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

/// The bridge's `tools/call` about `name`'s call `id` with `input`.
fn ask(n: u64, name: &str, id: &str, input: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": n, "method": "tools/call", "params": {
        "name": "permission",
        "arguments": {"tool_name": name, "input": input, "tool_use_id": id},
    }})
    .to_string()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_question_card_answers_in_the_calls_input_and_a_plan_is_denied_with_a_note() {
    let flow = Flow::with("native-cards", CARDS_FAKE, &stream());
    let plan = json!({"plan": "Do it.\nThen test it.", "planFilePath": "/plans/p.md"});
    let asks = [
        ask(2, "AskUserQuestion", "q1", &questions()),
        ask(3, "ExitPlanMode", "p1", &plan),
    ];
    flow.scratch
        .write("asks.jsonl", format!("{}\n", asks.join("\n")).as_bytes());
    let mut app = sent(&flow);
    app.wait_for("the calls", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("ExitPlanMode")
    });
    flow.scratch.write("ask", b"");

    // The questions, one at a time: a digit answers the first, and the second takes several.
    let first = card(&mut app, "the question card");
    assert!(first.starts_with("Claude asks (1 of 2)"), "{first}");
    assert!(first.contains("│ 2. Large: Large size"), "{first}");
    // The plan's call is asked while the question's card takes answers, as it was on a slow
    // runner: it waits behind the card, which takes its answer all the same.
    wait_for_answers(&mut app);
    flow.scratch.write("ask2", b"");
    std::thread::sleep(std::time::Duration::from_millis(700));
    answer_after_quiet(&mut app, "2");
    app.wait_for("the second question", |state| {
        text(state).starts_with("Claude asks (2 of 2)")
    });
    answer_after_quiet(&mut app, "1");
    answer_after_quiet(&mut app, "3");
    app.wait_for("two chosen", |state| {
        let text = text(state);
        text.contains("[x] Red") && text.contains("[x] Blue")
    });
    answer_after_quiet(&mut app, "RET");
    app.wait_for("the questions answered", in_composer);

    // The plan, as the request gives it, denied with a note.
    let plan = card(&mut app, "the plan card");
    assert!(
        plan.starts_with("Claude's plan, as its request gives it"),
        "{plan}"
    );
    assert!(plan.contains("│ Do it.\n│ Then test it."), "{plan}");
    answer_after_quiet(&mut app, "N");
    app.wait_for("the note's prompt", |state| {
        state["prompts"][0]["prompt"] == "Deny with a note: "
    });
    app.send_keys(&keys("not yet"));
    app.send_keys("RET");

    let answers: Vec<Value> = flow
        .wait_for_file("answers")
        .lines()
        .map(|line| {
            let answer: Value = serde_json::from_str(line).unwrap();
            let text = answer["result"]["content"][0]["text"].as_str().unwrap();
            serde_json::from_str(text).unwrap()
        })
        .collect();
    assert_eq!(answers[0]["behavior"], "allow");
    assert_eq!(
        answers[0]["updatedInput"]["answers"],
        json!({"Which size?": "Large", "Which colors?": "Red, Blue"}),
        "{}",
        answers[0]
    );
    assert_eq!(
        answers[0]["updatedInput"]["questions"],
        questions()["questions"]
    );
    assert_eq!(answers[1]["behavior"], "deny");
    assert!(
        answers[1]["message"].as_str().unwrap().ends_with("not yet"),
        "{}",
        answers[1]
    );
    let _ = fs::remove_file(flow.scratch.path().join("go"));
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
