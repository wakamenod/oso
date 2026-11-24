//! An agent's `open` shows no file while a native session's request waits in front of the person
//! (ADR 0165), with the cards' fake asking about two `ExitPlanMode` calls: it is refused with the
//! card, the composer, the transcript and the plan's edit in front, and shows the file once the
//! first is answered. While the second waits, in sight or not, highlighting stays as it is.
//!
//! Must not depend on: the app's internals; which buffers are in front has its own test.

use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::super::agent::{Agent, let_in};
use super::super::super::harness::App;
use super::super::super::worktrees::keys;
use super::cards::{CARDS_FAKE, ask};
use super::{Flow, answer_after_quiet, card, in_composer, plan, sent, text};

/// How long the channel waits after the person's last key, and a moment more.
const AFTER_TYPING: Duration = Duration::from_millis(1200);

/// The agent's `open` of `path`, once the person's last key is past.
fn open(agent: &mut Agent, path: &Path) -> Result<Value, String> {
    thread::sleep(AFTER_TYPING);
    agent.call("open", Some(json!({ "path": path, "line": null })))
}

/// The agent's `open` of `path` is refused, and the buffer named `front` stays in front.
fn refused(app: &mut App, agent: &mut Agent, path: &Path, front: &str) {
    let said = open(agent, path);
    let state = app.state();
    assert_eq!(state["buffer"]["name"], front, "{said:?}: {state}");
    let why = said.expect_err(front);
    assert!(why.contains("request waits"), "{front}: {why}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_agent_shows_no_file_while_a_request_waits_in_front() {
    let flow = Flow::with("native-front", CARDS_FAKE, &plan::stream());
    let plan = |text: &str| json!({"plan": text, "planFilePath": "/plans/p.md"});
    let asks = [
        ask(2, "ExitPlanMode", "p1", &plan("Write hello.txt.")),
        ask(3, "ExitPlanMode", "p2", &plan("Run the tests.")),
    ];
    flow.scratch
        .write("asks.jsonl", format!("{}\n", asks.join("\n")).as_bytes());
    let notes = flow.project.join("notes.md");
    let other = flow.scratch.write("other.txt", b"first\n");
    let mut app = sent(&flow);
    app.wait_for("the calls", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("ExitPlanMode")
    });
    let mut agent = let_in(&mut app);

    // A file open already, and one that is not, with the card in front.
    flow.scratch.write("ask", b"");
    card(&mut app, "the first plan's card");
    refused(&mut app, &mut agent, &notes, "*card: proj*");
    refused(&mut app, &mut agent, &other, "*card: proj*");
    // The session's composer and transcript, under which the card is.
    app.send_keys("C-x o");
    app.wait_for("the composer", in_composer);
    refused(&mut app, &mut agent, &notes, "*compose: proj*");
    app.send_keys("C-c C-o");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    refused(&mut app, &mut agent, &notes, "*claude: proj*");
    // The plan's edit, in place of the transcript.
    app.send_keys("C-x o");
    card(&mut app, "the card again");
    answer_after_quiet(&mut app, "e");
    app.wait_for("the plan's edit", |state| {
        state["buffer"]["name"] == "*plan: proj*"
    });
    refused(&mut app, &mut agent, &notes, "*plan: proj*");
    app.send_keys("C-c C-k");
    card(&mut app, "the card once more");
    answer_after_quiet(&mut app, "n");
    app.wait_for("the composer, the plan denied", in_composer);

    // Nothing waits: the file shows.
    let opened = open(&mut agent, &notes);
    assert_eq!(opened.expect("nothing waits")["name"], "notes.md");
    assert_eq!(app.state()["buffer"]["name"], "notes.md");

    // The second waits in a session out of sight: files show, and highlighting stays.
    flow.scratch.write("ask2", b"");
    app.wait_for("the session to need the person", |state| {
        state["sessions"].to_string().contains("needs input")
    });
    thread::sleep(AFTER_TYPING);
    let why = (agent.run("global-font-lock-mode", Some(0), "look")).expect_err("a request waits");
    assert!(why.contains("request waits"), "{why}");
    assert!(open(&mut agent, &other).is_ok());
    app.send_keys("C-x b");
    app.send_keys(&keys("*claude: proj*"));
    app.send_keys("RET");
    card(&mut app, "the second plan's card");
    answer_after_quiet(&mut app, "n");
    flow.wait_for_file("answers");
    thread::sleep(AFTER_TYPING);
    agent
        .run("global-font-lock-mode", Some(1), "look")
        .expect("nothing waits");
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
