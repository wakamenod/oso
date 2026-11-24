//! A native session's plan, edited before it is approved, and approved with a mode after it
//! (ADR 0165, part 3d), with the cards' fake asking about three `ExitPlanMode` calls. The first
//! is edited in `*plan: proj*`, where an approval at once only waits, and approved with auto by
//! `C-c C-A`, after `n` and then `y` to its question, its edit going back in the call's input.
//! The second's edit is approved by `C-c C-c`, which asks first too. The third's edit is given
//! up with `C-c C-k`, back to its card, where `a` approves it as Claude wrote it with accept
//! edits after it. Once the session stopped, its resume starts in accept edits, the mode the
//! person chose last (part 3b).
//!
//! Must not depend on: the app's internals; the answer's shape, the question's text and the
//! rows' marks have their own tests.

use std::fs;

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::worktrees::keys;
use super::cards::{CARDS_FAKE, ask};
use super::{Flow, MESSAGE, answer_after_quiet, card, card_saying, in_composer, sent, text};

/// The cards' fake, its bridge asked about three calls: the last two once the flow writes `ask2`.
fn fake() -> String {
    CARDS_FAKE.replace("for n in 2 3;", "for n in 2 3 4;")
}

/// The stream: the message replayed, and the three plans' calls, with no input, as the CLI
/// writes them (measured).
pub(super) fn stream() -> String {
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "tool_use", "id": "p1", "name": "ExitPlanMode", "input": {}},
            {"type": "tool_use", "id": "p2", "name": "ExitPlanMode", "input": {}},
            {"type": "tool_use", "id": "p3", "name": "ExitPlanMode", "input": {}},
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

/// Waits for the question asked before an approval of an edit, whose text starts with `says`.
fn asked(app: &mut App, what: &str, says: &str) {
    app.wait_for(what, |state| {
        state["prompts"][0]["prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.starts_with(says) && prompt.ends_with("(y or n)"))
    });
}

/// Sends `key` in the plan's edit once it takes an approval.
fn approve_after_quiet(app: &mut App, key: &str) {
    app.wait_for("the edit to take an approval", |state| {
        state["card"]["plan_takes_answer"] == true
    });
    app.send_keys(key);
}

/// Answers the question open `y` once it takes it.
fn yes_after_quiet(app: &mut App) {
    app.wait_for("the question to take its y", |state| {
        state["card"]["question_takes_answer"] == true
    });
    app.send_keys("y");
}

/// Answers the question open `n`, which leaves the edit in front.
fn no(app: &mut App) {
    app.send_keys("n");
    let state = app.wait_for("the question closed", |state| state["prompts"][0].is_null());
    assert_eq!(state["buffer"]["name"], "*plan: proj*");
}

/// An approval as the edit opens waits, or, on a machine too slow to be within the wait, asks.
/// What was typed is taken back.
fn an_approval_at_once_waits(app: &mut App) {
    app.send_keys("x C-c C-c");
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "*plan: proj*", "{state}");
    if state["prompts"][0].is_null() {
        assert_eq!(state["echo"], "Wait a moment, then approve the plan");
    } else {
        no(app);
    }
    app.send_keys("M-<");
    app.send_keys("C-d");
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

/// What the fake was told of the three plans: the first edited, with auto after it, the second
/// edited, and the third as Claude wrote it, with accept edits after it.
fn check_answers(flow: &Flow) {
    let said = answers(flow);
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
        json!({"behavior": "allow", "updatedInput": plan("Now Run the tests.")}),
    );
    assert_eq!(
        said[2],
        json!({
            "behavior": "allow",
            "updatedInput": plan("Tidy up."),
            "updatedPermissions": [
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            ],
        }),
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_plan_is_edited_before_it_is_approved_and_approved_with_a_mode() {
    let flow = Flow::with("native-plan", &fake(), &stream());
    let asks = [
        ask(2, "ExitPlanMode", "p1", &plan("Write hello.txt.")),
        ask(3, "ExitPlanMode", "p2", &plan("Run the tests.")),
        ask(4, "ExitPlanMode", "p3", &plan("Tidy up.")),
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
    assert!(first.contains(" e  edit"), "{first}");
    answer_after_quiet(&mut app, "e");
    assert_eq!(
        editing(&mut app, "the plan's edit").trim_end(),
        "Write hello.txt."
    );
    an_approval_at_once_waits(&mut app);
    app.send_keys("M->");
    app.send_keys(&format!("RET {}", keys("Then write pelican.txt.")));
    app.wait_for("the edit", |state| {
        text(state).trim_end().ends_with("pelican.txt.")
    });
    let auto = "Approve proj's plan as you edited it (+2 −1 lines) and leave it in auto mode";
    approve_after_quiet(&mut app, "C-c C-A");
    asked(&mut app, "the question before auto", auto);
    no(&mut app);
    approve_after_quiet(&mut app, "C-c C-A");
    asked(&mut app, "the question asked again", auto);
    yes_after_quiet(&mut app);
    app.wait_for("the edited plan approved", in_composer);

    // The second: its edit approved by `C-c C-c`, which asks too.
    flow.scratch.write("ask2", b"");
    let second = card(&mut app, "the second plan's card");
    assert!(second.contains("│ Run the tests."), "{second}");
    answer_after_quiet(&mut app, "e");
    editing(&mut app, "the second plan's edit");
    app.send_keys(&keys("Now "));
    let manual = "Approve proj's plan as you edited it (+1 −1 lines)? ";
    approve_after_quiet(&mut app, "C-c C-c");
    asked(&mut app, "the question before the edit", manual);
    no(&mut app);
    approve_after_quiet(&mut app, "C-c C-c");
    asked(&mut app, "that question again", manual);
    yes_after_quiet(&mut app);

    // The third: its edit given up, back to its card, which approves it with accept edits.
    let third = card_saying(&mut app, "the third plan's card", "│ Tidy up.");
    assert!(third.contains(" e  edit"), "{third}");
    answer_after_quiet(&mut app, "e");
    editing(&mut app, "the third plan's edit");
    app.send_keys(&keys("x"));
    app.send_keys("C-c C-k");
    let again = card(&mut app, "the third card again");
    assert!(again.contains("│ Tidy up."), "{again}");
    answer_after_quiet(&mut app, "a");

    check_answers(&flow);
    app.wait_for("the third approved", in_composer);
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
