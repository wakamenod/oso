//! A native session's long lines wrap (ADR 0165): a long Japanese reply shows its whole text in
//! the transcript, whose lines `toggle-truncate-lines` still cuts, and a long message shows
//! whole in the composer, which grows with it.
//!
//! Must not depend on: the app's internals; where rows break, and how a docked window's rows are
//! counted, have their own tests.

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::{FAKE, Flow, MESSAGE, in_composer, sent};

/// The text the selected window shows, its view rows one after the other.
fn joined(state: &Value) -> String {
    (state["text"]["rows"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect()
}

/// The rows the composer's window takes.
fn composer_rows(state: &Value) -> Option<u64> {
    (state["windows"].as_array()?.iter())
        .find(|window| window["docked"]["slot"] == "composer")?["area"]["rows"]
        .as_u64()
}

fn typed(app: &mut App, text: &str) {
    app.try_call("ime", Some(json!({ "commit": text })))
        .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_long_reply_and_a_long_message_show_whole() {
    let reply = "長い日本語の返事は窓の端で折り返して最後まで読める。".repeat(8);
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "text", "text": reply},
        ]}}),
        json!({"type": "result", "subtype": "success", "duration_ms": 10}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    let flow = Flow::with("native-wrap", FAKE, &format!("{}\n", lines.join("\n")));
    let mut app = sent(&flow);
    let state = app.wait_for("the whole reply", |state| {
        state["buffer"]["name"] == "*claude: proj*" && joined(state).contains(&reply)
    });
    assert_eq!(state["buffer"]["wrapped"], true);
    app.run_command("toggle-truncate-lines", None).unwrap();
    let state = app.state();
    assert_eq!(state["buffer"]["wrapped"], false, "the person's own toggle");
    // A row of the state holds its whole logical line, cut only as it is drawn.
    assert!(
        (state["text"]["rows"].as_array().unwrap().iter())
            .any(|row| row.as_str().is_some_and(|row| row.contains(&reply))),
        "one row a line"
    );
    app.run_command("toggle-truncate-lines", None).unwrap();

    // The composer: a row, then as many as its text wraps to, all of them shown.
    app.send_keys("i");
    let state = app.wait_for("the composer", in_composer);
    assert_eq!(state["buffer"]["wrapped"], true);
    assert_eq!(composer_rows(&state), Some(1));
    let message = "作文欄も長い行を折り返して全部見せる。".repeat(8);
    typed(&mut app, &message);
    let state = app.wait_for("the whole message", |state| joined(state) == message);
    let rows = composer_rows(&state).unwrap();
    assert!((2..=10).contains(&rows), "{rows} rows");
    assert_eq!(
        state["text"]["rows"].as_array().unwrap().len() as u64,
        rows,
        "each of its rows shows"
    );
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
