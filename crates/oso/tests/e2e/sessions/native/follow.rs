//! A native transcript follows the conversation's end (ADR 0165, part 1b-1) while its composer
//! is selected and while it is not shown, with a fake `claude -p` that replies to each message
//! with the lines the flow wrote; the wheel away from the end stops it, and the wheel back to
//! the end, or a send, starts it again.
//!
//! Must not depend on: the app's internals; which windows follow has its own tests.

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, in_composer, selected_composer, text};

/// The fake: for its input's line `N`, from 1, it replays the message, writes it to `sent.N`,
/// and once the flow writes `release.N`, gives `reply.N`, which the flow wrote, and the turn's
/// end. It exits once its input ends.
const FOLLOW_FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf '%s\n' '{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m"}'
n=0
while IFS= read -r line; do
  case "$line" in *'"control_request"'*) continue ;; esac
  n=$((n+1))
  said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
  printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
  printf '%s\n' "$said" > "$dir/sent.$n"
  while [ ! -f "$dir/release.$n" ]; do sleep 0.1; done
  cat "$dir/reply.$n"
  printf '{"type":"result","subtype":"success","duration_ms":10}\n'
done
"#;

/// The transcript's window, above its composer, which is selected.
fn transcript(state: &Value) -> &Value {
    let host = &selected_composer(state).expect("the composer")["docked"]["host"];
    (state["windows"].as_array().into_iter().flatten())
        .find(|window| window["docked"].is_null() && window["buffer"] == *host)
        .expect("the transcript's window")
}

/// Every window of the tree on the session's transcript, its composer selected.
fn transcript_windows(state: &Value) -> Vec<&Value> {
    let host = &selected_composer(state).expect("the composer")["docked"]["host"];
    (state["windows"].as_array().into_iter().flatten())
        .filter(|window| window["docked"].is_null() && window["buffer"] == *host)
        .collect()
}

fn top(state: &Value) -> u64 {
    transcript(state)["scroll"]["top"].as_u64().unwrap_or(0)
}

/// Writes the reply to message `n`, 60 lines, and sends `said`, which is it.
fn say(flow: &Flow, app: &mut App, n: usize, said: &str) {
    let lines: Vec<String> = (1..=60).map(|i| format!("reply {n} line {i}.")).collect();
    let reply = json!({ "type": "assistant", "message": { "id": format!("m{n}"), "content": [
        { "type": "text", "text": lines.join("\n") },
    ] } });
    (flow.scratch).write(&format!("reply.{n}"), format!("{reply}\n").as_bytes());
    app.send_keys(&keys(said));
    app.send_keys("C-c C-c");
    assert_eq!(flow.wait_for_file(&format!("sent.{n}")).trim(), said);
}

/// Wheels the transcript's window by `notches`, away from the end if positive.
fn wheel(app: &mut App, notches: i64) {
    let state = app.state();
    let area = &transcript(&state)["area"];
    let at = json!({ "row": area["row"], "column": area["column"].as_u64().unwrap_or(0) + 4 });
    app.call("wheel", Some(json!({ "lines": [0, notches], "at": at })));
}

/// The transcript, once its window, not selected, is past row `past`; then what it shows there,
/// without its view having moved.
fn shown_past(app: &mut App, past: u64) -> String {
    let state = app.wait_for("the transcript followed", |state| {
        in_composer(state) && top(state) > past
    });
    let before = top(&state);
    app.send_keys("C-c C-o");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert_eq!(state["scroll"]["top"], before, "it was there already");
    let shown = text(&state);
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    shown
}

/// A flow named `test` whose session runs, its composer selected.
fn started(test: &str) -> (Flow, App) {
    let flow = Flow::with(test, FOLLOW_FAKE, "");
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        (state["modeline"].as_str()).is_some_and(|modeline| !modeline.contains("restricted"))
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    (flow, app)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_transcript_follows_behind_its_composer_until_the_wheel_leaves_its_end() {
    let (flow, mut app) = started("native-follow");
    say(&flow, &mut app, 1, "one");
    assert_eq!(top(&app.state()), 0);
    super::end_turn(&flow, &mut app, "release.1");
    let shown = shown_past(&mut app, 0);
    assert!(shown.contains("reply 1 line 60."), "{shown}");

    // The wheel leaves the end: the turn's rows move nothing, and the wheel back follows again.
    say(&flow, &mut app, 2, "two");
    wheel(&mut app, 4);
    let away = top(&app.state());
    super::end_turn(&flow, &mut app, "release.2");
    assert_eq!(top(&app.state()), away, "a reader is not moved");
    wheel(&mut app, -100);
    let shown = shown_past(&mut app, away);
    assert!(shown.contains("reply 2 line 60."), "{shown}");

    // So does a send.
    wheel(&mut app, 8);
    let away = top(&app.state());
    say(&flow, &mut app, 3, "three");
    super::end_turn(&flow, &mut app, "release.3");
    let shown = shown_past(&mut app, away + 60);
    assert!(shown.contains("reply 3 line 60."), "{shown}");

    // Rows that come while the window shows another buffer, from the end it left.
    say(&flow, &mut app, 4, "four");
    app.open_file(&flow.project.join("notes.md"));
    super::end_turn(&flow, &mut app, "release.4");
    super::restart::show_transcript(&mut app);
    let state = app.wait_for("the transcript's end", |state| {
        text(state).contains("reply 4 line 60.")
    });
    assert_eq!(state["buffer"]["name"], "*claude: proj*");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_send_beside_a_reader_moves_the_carrier_alone_and_point_keeps_its_queued_message() {
    let (flow, mut app) = started("native-follow-beside");
    say(&flow, &mut app, 1, "one");
    super::end_turn(&flow, &mut app, "release.1");
    shown_past(&mut app, 0);

    // Side by side, the window that carries the composer follows a send; a reader stays.
    app.send_keys("C-x 3");
    app.send_keys("i");
    let state = app.wait_for("the composer", in_composer);
    let carrier = transcript_windows(&state)
        .into_iter()
        .find(|window| {
            window["area"]["column"] == selected_composer(&state).unwrap()["area"]["column"]
        })
        .expect("the carrier")["id"]
        .clone();
    for window in transcript_windows(&state) {
        let at = json!({ "row": window["area"]["row"], "column": window["area"]["column"] });
        app.call("wheel", Some(json!({ "lines": [0, 8], "at": at })));
    }
    let before = app.state();
    say(&flow, &mut app, 2, "two");
    super::end_turn(&flow, &mut app, "release.2");
    let top_of = |state: &Value, id: &Value| {
        (transcript_windows(state).into_iter())
            .find(|window| window["id"] == *id)
            .and_then(|window| window["scroll"]["top"].as_u64())
            .unwrap_or(0)
    };
    let after = app.wait_for("the carrier followed", |state| {
        top_of(state, &carrier) > top_of(&before, &carrier) + 50
    });
    let reader = transcript_windows(&before)
        .into_iter()
        .find(|window| window["id"] != carrier)
        .expect("the reader")["id"]
        .clone();
    assert_eq!(top_of(&after, &reader), top_of(&before, &reader));
    app.send_keys("C-c C-o");
    app.send_keys("C-x 1");
    app.send_keys("i");
    app.wait_for("the composer", in_composer);

    // Point on a queued message drops that one alone, never the one its row is now.
    say(&flow, &mut app, 3, "three");
    for queued in ["left", "gone"] {
        app.send_keys(&keys(queued));
        app.send_keys("C-c C-c");
    }
    app.send_keys("C-c C-q");
    app.send_keys("C-n");
    app.send_keys("d");
    app.send_keys("d");
    let state = app.wait_for("one queued", |state| text(state).contains("Queued (1)"));
    let shown = text(&state);
    assert!(
        shown.contains("1. left") && !shown.contains("gone"),
        "{shown}"
    );
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
