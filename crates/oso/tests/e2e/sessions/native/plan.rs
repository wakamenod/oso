//! A native session's plan, edited before it is approved, and approved with a mode after it
//! (ADR 0165, part 3d), with the cards' fake asking about two `ExitPlanMode` calls: the first
//! is edited in `*plan: proj*` and approved with auto by `C-c C-A`, after `n` and then `y` to
//! its question, its edit going back in the call's input; the second's edit is given up with `C-c C-k`, back to its card, where `a` approves
//! it as Claude wrote it with accept edits after it. Once the session stopped, its resume
//! starts in accept edits, the mode the person chose last (part 3b).
//!
//! Must not depend on: the app's internals; the answer's shape and the rows' marks have their
//! own tests.

use std::fs;

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::worktrees::keys;
use super::cards::{CARDS_FAKE, ask};
use super::{Flow, MESSAGE, answer_after_quiet, card, in_composer, sent, text};

/// The stream: the message replayed, and the two plans' calls, with no input, as the CLI
/// writes them (measured).
fn stream() -> String {
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "tool_use", "id": "p1", "name": "ExitPlanMode", "input": {}},
            {"type": "tool_use", "id": "p2", "name": "ExitPlanMode", "input": {}},
        ]}}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

fn plan(text: &str) -> Value {
    json!({"plan": text, "planFilePath": "/plans/p.md"})
}

/// Waits for `*plan: proj*` in place of the transcript: its text.
fn editing(app: &mut App, what: &str) -> String {
    let state = app.wait_for(what, |state| state["buffer"]["name"] == "*plan: proj*");
    text(&state)
}

/// Waits for the question asked before an approval with auto.
fn asked(app: &mut App, what: &str) {
    app.wait_for(what, |state| {
        state["prompts"][0]["prompt"]
            .as_str()
            .is_some_and(|prompt| {
                prompt.starts_with("Approve proj's plan as you edited it and leave it in auto mode")
                    && prompt.ends_with("(y or n)")
            })
    });
}

/// What the bridge told the fake, as the CLI reads it.
fn answers(flow: &Flow) -> Vec<Value> {
    flow.wait_for_file("answers")
        .lines()
        .map(|line| {
            let answer: Value = serde_json::from_str(line).unwrap();
            let text = answer["result"]["content"][0]["text"].as_str().unwrap();
            serde_json::from_str(text).unwrap()
        })
        .collect()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_plan_is_edited_before_it_is_approved_and_approved_with_a_mode() {
    let flow = Flow::with("native-plan", CARDS_FAKE, &stream());
    let asks = [
        ask(2, "ExitPlanMode", "p1", &plan("Write hello.txt.")),
        ask(3, "ExitPlanMode", "p2", &plan("Run the tests.")),
    ];
    flow.scratch
        .write("asks.jsonl", format!("{}\n", asks.join("\n")).as_bytes());
    let mut app = sent(&flow);
    app.wait_for("the calls", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("ExitPlanMode")
    });
    flow.scratch.write("ask", b"");

    // `e` shows Claude's plan, to edit, in place of the transcript; `C-c C-A` approves the edit
    // with auto, once `y` answered its question.
    let first = card(&mut app, "the first plan's card");
    assert!(first.contains("│ Write hello.txt."), "{first}");
    assert!(first.contains("[e] edit"), "{first}");
    answer_after_quiet(&mut app, "e");
    assert_eq!(
        editing(&mut app, "the plan's edit").trim_end(),
        "Write hello.txt."
    );
    app.send_keys("M->");
    app.send_keys(&format!("RET {}", keys("Then write pelican.txt.")));
    app.wait_for("the edit", |state| {
        text(state).trim_end().ends_with("pelican.txt.")
    });
    app.send_keys("C-c C-A");
    asked(&mut app, "the question before auto");
    app.send_keys("n");
    let state = app.wait_for("the question closed", |state| state["prompts"][0].is_null());
    assert_eq!(state["buffer"]["name"], "*plan: proj*");
    app.send_keys("C-c C-A");
    asked(&mut app, "the question asked again");
    app.wait_for("the question to take its y", |state| {
        state["card"]["question_takes_answer"] == true
    });
    app.send_keys("y");
    app.wait_for("the edited plan approved", in_composer);

    // The second: its edit given up, back to its card, which approves it with accept edits.
    flow.scratch.write("ask2", b"");
    let second = card(&mut app, "the second plan's card");
    assert!(second.contains("│ Run the tests."), "{second}");
    answer_after_quiet(&mut app, "e");
    editing(&mut app, "the second plan's edit");
    app.send_keys(&keys("x"));
    app.send_keys("C-c C-k");
    let again = card(&mut app, "the second card again");
    assert!(again.contains("│ Run the tests."), "{again}");
    answer_after_quiet(&mut app, "a");

    let said = answers(&flow);
    assert_eq!(
        said[0],
        json!({
            "behavior": "allow",
            "updatedInput": plan("Write hello.txt.\nThen write pelican.txt."),
            "updatedPermissions": [
                {"type": "setMode", "mode": "auto", "destination": "session"},
            ],
        }),
    );
    assert_eq!(
        said[1],
        json!({
            "behavior": "allow",
            "updatedInput": plan("Run the tests."),
            "updatedPermissions": [
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            ],
        }),
    );
    app.wait_for("the second approved", in_composer);
    let _ = fs::remove_file(flow.scratch.path().join("go"));
    flow.scratch.write("go", b"0");
    the_next_process_starts_in_the_approved_mode(&flow, &mut app);
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// The fake's runs' arguments, by its `run.PID` files.
fn runs(flow: &Flow) -> Vec<String> {
    let entries = fs::read_dir(flow.scratch.path()).unwrap();
    let names = entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok());
    let runs = names.filter(|name| name.starts_with("run."));
    runs.filter_map(|name| fs::read_to_string(flow.scratch.path().join(name)).ok())
        .collect()
}

/// Once the session stopped, its resume is given the mode its last plan was approved with,
/// `a`'s accept edits, as `--resume` keeps none (part 3b); its first process was given none.
fn the_next_process_starts_in_the_approved_mode(flow: &Flow, app: &mut App) {
    app.send_keys("C-c C-o");
    app.wait_for("the session stopped", |state| {
        state["buffer"]["name"] == "*claude: proj*"
            && state["modeline"]
                .as_str()
                .is_some_and(|modeline| modeline.contains("stopped"))
    });
    let first = runs(flow);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(!first[0].contains("--permission-mode"), "{}", first[0]);
    app.run_command("session-resume", None).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let resumed = loop {
        let runs = runs(flow);
        if let Some(run) = runs.iter().find(|run| run.contains("arg --resume=")) {
            break run.clone();
        }
        assert!(std::time::Instant::now() < deadline, "no resume: {runs:?}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert!(
        resumed.contains("arg --permission-mode=acceptEdits"),
        "{resumed}"
    );
}
