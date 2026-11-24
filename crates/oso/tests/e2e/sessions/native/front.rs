//! What an agent may not do while a native session's request waits (ADR 0182, "Point" 8), with the
//! cards' fake asking about two `ExitPlanMode` calls: its `open` and its commands, a look among
//! them, are refused with the card, the composer, the transcript and the plan's edit in front, and
//! run once the first is answered. While the second waits out of sight, highlighting, wrapping and
//! the session shown stay as they are; in a window beside the selected one, the windows too.
//!
//! Must not depend on: the app's internals; which buffers are in front has its own test.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::super::agent::{Agent, let_in};
use super::super::super::harness::App;
use super::super::super::worktrees::keys;
use super::cards::{CARDS_FAKE, ask};
use super::{Flow, answer_after_quiet, card, in_composer, plan, sent, text};

/// What `call` is answered once the channel says no more that the person is typing: for a second
/// after a key, and on X11 after an input method's report too, which comes when it comes.
fn answer(
    agent: &mut Agent,
    call: impl Fn(&mut Agent) -> Result<Value, String>,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        match call(agent) {
            Err(why) if why.contains("person is typing") => {
                assert!(started.elapsed() < Duration::from_secs(20), "still: {why}");
                thread::sleep(Duration::from_millis(200));
            }
            answer => return answer,
        }
    }
}

/// The agent's `open` of `path`.
fn open(agent: &mut Agent, path: &Path) -> Result<Value, String> {
    answer(agent, |agent| {
        agent.call("open", Some(json!({ "path": path, "line": null })))
    })
}

/// The agent's look `name`, given `arg`.
fn look(agent: &mut Agent, name: &str, arg: Option<i64>) -> Result<Value, String> {
    answer(agent, |agent| agent.run(name, arg, "look"))
}

/// The agent's look `name` is refused, as a request waits.
fn held(app: &mut App, agent: &mut Agent, name: &str) {
    let said = look(agent, name, None);
    let why = said.expect_err(name);
    assert!(
        why.contains("request waits"),
        "{name}: {why}: {}",
        app.state()
    );
}

/// The agent's `open` of `path` is refused, and the buffer named `front` stays in front.
fn refused(app: &mut App, agent: &mut Agent, path: &Path, front: &str) {
    let said = open(agent, path);
    let state = app.state();
    assert_eq!(state["buffer"]["name"], front, "{said:?}: {state}");
    let why = said.expect_err(front);
    assert!(why.contains("request waits"), "{front}: {why}");
    for name in ["forward-char", "scroll-up-command", "previous-buffer"] {
        held(app, agent, name);
    }
    let state = app.state();
    assert_eq!(state["buffer"]["name"], front, "{state}");
}

/// With the request's transcript in a window beside the selected one, an agent moves in the
/// selected one and shows files there, and changes no window. Point is in the card before and after.
fn in_sight(app: &mut App, agent: &mut Agent, path: &Path) {
    // The transcript in the new window, the other window selected.
    app.send_keys("C-x o");
    app.wait_for("the composer", in_composer);
    app.send_keys("C-c C-o");
    app.send_keys("C-x 3");
    app.send_keys("C-x b");
    app.send_keys(&keys("other.txt"));
    app.send_keys("RET");
    app.wait_for("the file beside the transcript", |state| {
        state["buffer"]["name"] == "other.txt"
    });
    look(agent, "forward-char", None).expect("in the file's window");
    let opened = open(agent, path);
    assert_eq!(opened.expect("in the file's window")["name"], "notes.md");
    for name in ["other-window", "delete-other-windows", "enlarge-window"] {
        held(app, agent, name);
    }
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "notes.md", "{state}");
    // Round the transcript's window to its card.
    app.send_keys("C-x 0");
    app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.send_keys("C-x o");
    card(app, "the second plan's card again");
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
    for name in ["global-font-lock-mode", "toggle-truncate-lines"] {
        held(&mut app, &mut agent, name);
    }
    look(&mut agent, "forward-char", None).expect("the request is out of sight");
    open(&mut agent, &other).expect("the request is out of sight");
    app.send_keys("C-x b");
    app.send_keys(&keys("*claude: proj*"));
    app.send_keys("RET");
    card(&mut app, "the second plan's card");
    in_sight(&mut app, &mut agent, &notes);
    answer_after_quiet(&mut app, "n");
    flow.wait_for_file("answers");
    look(&mut agent, "global-font-lock-mode", Some(1)).expect("nothing waits");
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
