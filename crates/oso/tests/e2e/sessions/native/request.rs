//! A native session's call that waits for the person (ADR 0182), with the cards' fake asking about
//! an edit taller than the window and a command: the block is the last text of the conversation,
//! what the person types in the composer is text; `C-c C-j` goes to the line, where `y` shows what was not read and allows only once all of it was, `C-u y` asks first, and
//! its question closes as oso's window loses the front. While it waits the session does not switch
//! to the TUI and a restart waits; and once its bridge goes, so does the block.
//!
//! Must not depend on: the app's internals; what counts as read, and when a key answers, have their
//! own tests.

use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::worktrees::keys;
use super::cards::{CARDS_FAKE, ask};
use super::{Flow, MESSAGE, in_composer, sent, text};

/// Whether a card or a request's line is docked in a window.
fn docked(state: &Value) -> bool {
    (state["windows"].as_array())
        .is_some_and(|windows| windows.iter().any(|w| w["docked"]["slot"] == "card"))
}

/// Whether point is on the session's request's line.
pub(super) fn on_line(state: &Value) -> bool {
    state["buffer"]["name"] == "*request: proj*"
}

/// The person's input source is a Japanese one, or not, whatever the keyboard's is.
pub(super) fn japanese(app: &mut App, japanese: bool) {
    app.try_call("input-source", Some(json!({ "japanese": japanese })))
        .unwrap();
}

/// Waits until a request's line is docked, as oso's window is in front.
pub(super) fn request(app: &mut App, what: &str) {
    app.try_call("focus", Some(json!({ "focused": true })))
        .unwrap();
    japanese(app, false);
    app.wait_for(what, |state| docked(state) && !state["request"].is_null());
}

/// `C-c C-j`: point goes to the request's line.
pub(super) fn go(app: &mut App) {
    app.send_keys("C-c C-j");
    app.wait_for("point on the line", on_line);
}

/// Sends `key` once the line takes an answer, again while it says to wait, as `answer_after_quiet`
/// does on a card.
pub(super) fn answer(app: &mut App, key: &str) {
    for _ in 0..3 {
        app.wait_for("the line to take an answer", |state| {
            state["request"]["takes_answer"] == true
        });
        app.send_keys(key);
        if app.state()["echo"] != "Wait a moment, then answer the request" {
            return;
        }
    }
    panic!(
        "the line took no {key}, sent three times as it took answers: {}",
        app.state()
    );
}

/// Allows the call whose line point is on, once what its window shows of it was read.
pub(super) fn allow(app: &mut App) {
    app.wait_for("all of it to be read", |state| {
        state["request"]["allows"] == true
    });
    answer(app, "y");
}

/// Sends `key` to the open question once it takes an answer.
fn answer_question(app: &mut App, key: &str) {
    app.wait_for("the question to take an answer", |state| {
        state["card"]["question_takes_answer"] == true
    });
    app.send_keys(key);
}

/// Reads on, page by page, until all of it was read and its end is in view.
pub(super) fn read_on(app: &mut App) {
    let started = Instant::now();
    loop {
        // What is in view is read first, whatever the window system said meanwhile of a screen the
        // person uses.
        app.try_call("focus", Some(json!({ "focused": true })))
            .unwrap();
        thread::sleep(Duration::from_millis(900));
        let state = app.state();
        if state["request"]["allows"] == true {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "not read: {}",
            state["request"]
        );
        app.send_keys("SPC");
    }
}

/// How many of the request's rows were read.
fn read(state: &Value) -> u64 {
    state["request"]["read"].as_u64().unwrap_or_default()
}

fn edit() -> Value {
    let old: String = (0..80).map(|n| format!("old line {n}\n")).collect();
    let new: String = (0..80).map(|n| format!("new line {n}\n")).collect();
    json!({"file_path": "/w/notes.md", "old_string": old, "new_string": new})
}

fn command() -> Value {
    json!({"command": "make", "description": "Build it", "timeout": 600_000})
}

fn stream() -> String {
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "tool_use", "id": "e1", "name": "Edit", "input": edit()},
            {"type": "tool_use", "id": "b1", "name": "Bash", "input": command()},
        ]}}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

/// A flow whose fake asks about the edit once the flow writes `ask`, and about the command once the flow writes `ask2`.
pub(super) fn asking(name: &str) -> (Flow, App) {
    let flow = Flow::with(name, CARDS_FAKE, &stream());
    let asks = [
        ask(2, "Edit", "e1", &edit()),
        ask(3, "Bash", "b1", &command()),
    ];
    flow.scratch
        .write("asks.jsonl", format!("{}\n", asks.join("\n")).as_bytes());
    let mut app = sent(&flow);
    app.wait_for("the calls", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("Bash")
    });
    (flow, app)
}

/// What the bridge told the fake of each call.
pub(super) fn answers(flow: &Flow) -> Vec<Value> {
    (flow.wait_for_file("answers").lines())
        .map(|line| {
            let answer: Value = serde_json::from_str(line).unwrap();
            let said = answer["result"]["content"][0]["text"].as_str().unwrap();
            serde_json::from_str(said).unwrap()
        })
        .collect()
}

/// While a request waits, the session does not switch to the TUI, and a restart waits.
fn held_while_it_waits(app: &mut App) {
    app.run_command("session-switch-mode", None).unwrap();
    let state = app.state();
    assert!(docked(&state), "{state}");
    assert_eq!(state["buffer"]["name"], "*claude: proj*", "{state}");
    app.run_command("oso-restart", None).unwrap();
    app.wait_for("the restart to wait", |state| {
        (state["echo"].as_str()).is_some_and(|echo| echo.contains("oso-restart waits"))
    });
    app.send_keys("C-g");
    app.wait_for("the restart cancelled", |state| {
        (state["echo"].as_str()).is_some_and(|echo| !echo.contains("waits"))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_call_is_read_in_the_conversation_and_allowed_from_its_line() {
    let (flow, mut app) = asking("native-request");
    flow.scratch.write("ask", b"");
    request(&mut app, "the edit's block");
    // Point goes to its line from the transcript's end, and back with `C-g`; the block is the
    // transcript's last text.
    app.wait_for("point on the line", on_line);
    app.send_keys("C-g");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert!(
        text(&state).starts_with("⚠ Edit asks to change a file"),
        "{state}"
    );
    assert_eq!(
        state["request"]["line"], " Edit asks to change a file   C-c C-j  answers it",
        "{state}"
    );
    held_while_it_waits(&mut app);

    // A letter in the composer is text.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    thread::sleep(Duration::from_millis(700));
    app.send_keys(&keys("y then"));
    app.wait_for("the draft", |state| text(state) == "y then");
    assert!(docked(&app.state()));

    // `y` on the line shows what was not read, and answers nothing.
    go(&mut app);
    app.wait_for("its end to be read", |state| read(state) > 10);
    answer(&mut app, "y");
    let state = app.state();
    assert!(docked(&state), "{state}");
    assert!(
        (state["echo"].as_str()).is_some_and(|echo| echo.starts_with("Not allowed: here is")),
        "{state}"
    );
    let before = read(&state);
    app.wait_for("its start to be read", |state| read(state) > before);
    read_on(&mut app);
    allow(&mut app);
    // Answered, point goes to the draft, as it was.
    let state = app.wait_for("the draft again", in_composer);
    assert_eq!(text(&state), "y then");
    assert!(!docked(&state), "{state}");

    // The command, unread: the question says so, and closes with the front.
    flow.scratch.write("ask2", b"");
    request(&mut app, "the command's block");
    assert!(in_composer(&app.state()));
    go(&mut app);
    app.wait_for("some of it in view", |state| {
        state["request"]["may_ask"] == true
    });
    answer(&mut app, "RET");
    let state = app.wait_for("its input", |state| {
        state["buffer"]["name"] == "*result: proj*"
    });
    assert!(text(&state).contains("\"timeout\": 600000"), "{state}");
    app.send_keys("C-x b");
    app.send_keys(&keys("*claude: proj*"));
    app.send_keys("RET");
    go(&mut app);
    allow(&mut app);

    let said = answers(&flow);
    assert_eq!(
        said[0],
        json!({"behavior": "allow", "updatedInput": edit()})
    );
    assert_eq!(
        said[1],
        json!({"behavior": "allow", "updatedInput": command()})
    );
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_call_not_read_is_allowed_after_a_question_and_one_that_goes_takes_its_block() {
    let (flow, mut app) = asking("native-request-unread");
    flow.scratch.write("ask", b"");
    request(&mut app, "the edit's block");
    go(&mut app);
    app.wait_for("some of it in view", |state| {
        state["request"]["may_ask"] == true
    });
    answer(&mut app, "C-u y");
    let state = app.state();
    let asked = state["prompts"][0]["prompt"].as_str().unwrap_or_default();
    assert!(
        asked.starts_with("Allow Edit in proj unread: ") && asked.contains("rows are not read"),
        "{state}"
    );
    // It closes as oso's window loses the front.
    app.try_call("focus", Some(json!({ "focused": false })))
        .unwrap();
    app.wait_for("no question", |state| {
        state["prompts"].as_array().is_some_and(Vec::is_empty)
    });
    let state = app.state();
    assert_eq!(state["request"]["takes_answer"], false, "{state}");
    app.try_call("focus", Some(json!({ "focused": true })))
        .unwrap();
    app.wait_for("some of it in view", |state| {
        state["request"]["may_ask"] == true
    });
    answer(&mut app, "C-u y");
    answer_question(&mut app, "n");
    assert!(docked(&app.state()), "n answers nothing");
    answer(&mut app, "C-u y");
    answer_question(&mut app, "y");
    app.wait_for("the edit answered", |state| !docked(state));

    // The command's bridge goes while point is on its line.
    flow.scratch.write("ask2", b"");
    request(&mut app, "the command's block");
    go(&mut app);
    flow.scratch.write("answers", b"gone\n");
    let state = app.wait_for("the block gone", |state| !docked(state));
    assert!(in_composer(&state), "{state}");
    app.send_keys("C-c C-o");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert!(!text(&state).contains("asks to run"), "{state}");
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
}
