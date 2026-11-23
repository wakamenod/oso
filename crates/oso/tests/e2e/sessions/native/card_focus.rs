//! A native session's card waits for the person (ADR 0180), with the cards' fake: one that comes
//! during a search in the transcript, and one during a composition in the empty composer, leave
//! point where it is, however long the person pauses, and take it once that is over. An answer
//! ends each.
//!
//! Must not depend on: the app's internals; what a card waits for has its own tests.

use std::time::Duration;

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::cards::{CARDS_FAKE, ask, questions, stream};
use super::{Flow, answer_after_quiet, card, in_composer, sent, text};

/// Longer than a card waits with no key before point goes to it.
const A_PAUSE: Duration = Duration::from_millis(1200);

/// Waits until a card is docked, and then as long as one that could take point would.
fn docked_and_paused(app: &mut App, what: &str) -> Value {
    app.wait_for(what, |state| {
        (state["windows"].as_array())
            .is_some_and(|windows| windows.iter().any(|w| w["docked"]["slot"] == "card"))
    });
    std::thread::sleep(A_PAUSE);
    app.state()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_card_waits_while_the_person_searches_or_composes() {
    let flow = Flow::with("native-card-focus", CARDS_FAKE, &stream());
    let plan = json!({"plan": "Do it.", "planFilePath": "/plans/p.md"});
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

    // A search in the transcript.
    app.send_keys("C-s");
    app.wait_for("the search", |state| !state["isearch"].is_null());
    flow.scratch.write("ask", b"");
    let state = docked_and_paused(&mut app, "the question's card");
    assert_eq!(state["buffer"]["name"], "*claude: proj*");
    assert!(!state["isearch"].is_null(), "the search goes on");
    app.send_keys("RET");
    card(&mut app, "the card, the search over");
    answer_after_quiet(&mut app, "n");
    app.wait_for("the questions declined", in_composer);

    // A composition in the composer, which holds no text yet.
    app.try_call("ime", Some(json!({ "preedit": "に", "range": [3, 3] })))
        .unwrap();
    flow.scratch.write("ask2", b"");
    let state = docked_and_paused(&mut app, "the plan's card");
    assert!(in_composer(&state), "{state}");
    assert_eq!(state["ime"]["composing"], true, "the composition goes on");
    app.try_call("ime", Some(json!({ "preedit": "" }))).unwrap();
    card(&mut app, "the card, the composition over");
    answer_after_quiet(&mut app, "n");
    flow.wait_for_file("answers");
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
