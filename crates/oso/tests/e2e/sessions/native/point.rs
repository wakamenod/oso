//! Point and a native session's call (ADR 0182, "Point"), with the cards' fake asking about an edit
//! taller than the window and a command: point goes to the line by itself from the empty composer,
//! whose place the line takes; what is typed as it goes is a draft there; not with a Japanese input
//! source, a draft, or the composer chosen; and a tall block shows its head.
//!
//! Must not depend on: the app's internals. The input source is the dev socket's, never the
//! keyboard's. Text typed "as point comes" is typed with oso's window not in front, where the line
//! takes no key whatever the time, so that no flow races half a second.

use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::worktrees::keys;
use super::request::{allow, answer, answers, asking, go, japanese, on_line, read_on};
use super::{Flow, in_composer, text};

/// Longer than point waits before it goes by itself.
const LONG: Duration = Duration::from_millis(1500);

fn front(app: &mut App, focused: bool) {
    app.try_call("focus", Some(json!({ "focused": focused })))
        .unwrap();
}

/// Waits until point went to the line by itself, oso's window in front and its source no Japanese one
/// again and again, whatever the window system says meanwhile of a screen the person uses.
fn comes(app: &mut App, what: &str) -> Value {
    let mut state = Value::Null;
    for _ in 0..80 {
        front(app, true);
        japanese(app, false);
        thread::sleep(Duration::from_millis(250));
        state = app.state();
        if on_line(&state) {
            return state;
        }
    }
    panic!(
        "no {what}: focused {}, request {}, buffer {}",
        state["sessions"]["focused"], state["request"], state["buffer"]["name"]
    );
}

/// The slots docked in a window that shows the session, as drawn.
fn slots(state: &Value) -> Vec<String> {
    (state["windows"].as_array().into_iter().flatten())
        .filter(|window| window["area"]["rows"].as_u64().unwrap_or(0) > 0)
        .filter_map(|window| window["docked"]["slot"].as_str().map(str::to_owned))
        .collect()
}

/// The session's flow, point in its empty composer, oso's window in front for a while.
fn in_empty_composer(name: &str) -> (Flow, App) {
    let (flow, mut app) = asking(name);
    front(&mut app, true);
    japanese(&mut app, false);
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    thread::sleep(LONG);
    (flow, app)
}

/// The request is docked, and point stayed in the composer, which holds `draft`.
fn stays(app: &mut App, what: &str, draft: &str) {
    app.wait_for(what, |state| slots(state).contains(&"card".to_owned()));
    thread::sleep(LONG);
    let state = app.state();
    assert!(in_composer(&state), "{what}: {state}");
    assert_eq!(text(&state), draft, "{what}: {state}");
}

fn end(flow: &Flow, mut app: App) {
    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn point_goes_to_the_line_in_the_empty_composers_place_and_text_typed_as_it_goes_is_a_draft() {
    let (flow, mut app) = in_empty_composer("native-point");
    flow.scratch.write("ask", b"");
    let state = comes(&mut app, "point on the line");
    assert_eq!(slots(&state), ["card"], "in the composer's place: {state}");

    // Typed as it goes: the composer is back, with it, and nothing is answered.
    front(&mut app, false);
    app.send_keys("y");
    let state = app.wait_for("the draft", in_composer);
    assert_eq!(text(&state), "y", "{state}");
    assert_eq!(slots(&state), ["card", "composer"], "{state}");
    app.send_keys(&keys("es please"));
    front(&mut app, true);
    stays(&mut app, "the draft kept", "yes please");
    // Cleared, half a second with no key and point goes again.
    app.send_keys("C-a C-k");
    let state = comes(&mut app, "point on the line again");
    assert_eq!(slots(&state), ["card"], "{state}");

    // RET as it goes does nothing.
    front(&mut app, false);
    app.send_keys("RET");
    let state = app.state();
    assert!(on_line(&state), "{state}");
    assert_eq!(state["echo"], "Wait a moment, then answer the request");
    front(&mut app, true);

    read_on(&mut app);
    allow(&mut app);
    let state = app.wait_for("the composer, empty", in_composer);
    assert_eq!(
        (text(&state).as_str(), slots(&state)),
        ("", vec!["composer".to_owned()])
    );

    // The next request, right after: an impatient second `y` is a draft, and answers nothing.
    flow.scratch.write("ask2", b"");
    comes(&mut app, "point on the next line");
    front(&mut app, false);
    app.send_keys("y");
    let state = app.wait_for("the second y", in_composer);
    assert_eq!(text(&state), "y", "{state}");
    front(&mut app, true);
    stays(&mut app, "the second y kept", "y");
    app.send_keys("DEL");
    comes(&mut app, "point on the line once more");
    app.wait_for("the line to take an answer", |state| {
        state["request"]["takes_answer"] == true
    });
    // `i` is the composer, from which point does not go by itself, sent or cleared.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("x"));
    app.send_keys("DEL");
    stays(&mut app, "the composer chosen", "");
    go(&mut app);
    assert_eq!(slots(&app.state()), ["card"]);
    allow(&mut app);
    assert_eq!(answers(&flow).len(), 2);
    end(&flow, app);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn with_a_japanese_source_the_person_goes_to_the_line() {
    let (flow, mut app) = in_empty_composer("native-point-japanese");
    japanese(&mut app, true);
    thread::sleep(LONG);
    flow.scratch.write("ask", b"");
    stays(&mut app, "a Japanese source", "");
    assert_eq!(slots(&app.state()), ["card", "composer"]);
    // A prompt's ASCII source is not the person's.
    app.send_keys("M-x");
    japanese(&mut app, false);
    app.send_keys("C-g");
    stays(&mut app, "after a prompt", "");
    // Another source, and half a second: point goes.
    thread::sleep(LONG);
    japanese(&mut app, false);
    japanese(&mut app, true);
    japanese(&mut app, false);
    comes(&mut app, "point on the line");
    // Japanese told of late, the person having done nothing since: point goes back.
    japanese(&mut app, true);
    let state = app.wait_for("the composer again", in_composer);
    assert_eq!(slots(&state), ["card", "composer"], "{state}");
    stays(&mut app, "Japanese told late", "");
    // There by their own doing, point stays.
    go(&mut app);
    japanese(&mut app, false);
    japanese(&mut app, true);
    thread::sleep(LONG);
    assert!(on_line(&app.state()));
    answer(&mut app, "n");
    app.wait_for("the composer", in_composer);
    flow.scratch.write("ask2", b"");
    stays(&mut app, "the next request", "");
    go(&mut app);
    allow(&mut app);
    assert_eq!(answers(&flow).len(), 2);
    end(&flow, app);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_draft_stays_where_it_is_until_it_is_cleared() {
    let (flow, mut app) = in_empty_composer("native-point-draft");
    app.send_keys(&keys("wait, y"));
    flow.scratch.write("ask", b"");
    stays(&mut app, "a draft", "wait, y");
    assert_eq!(slots(&app.state()), ["card", "composer"]);
    // Over a draft the line stands above it, and an answer leads back to it.
    go(&mut app);
    assert_eq!(slots(&app.state()), ["card", "composer"]);
    app.wait_for("some of it read", |state| {
        state["request"]["may_ask"] == true
    });
    answer(&mut app, "n");
    let state = app.wait_for("the draft", in_composer);
    assert_eq!(text(&state), "wait, y", "{state}");
    flow.scratch.write("ask2", b"");
    stays(&mut app, "the next, over the draft", "wait, y");
    app.send_keys("C-a C-k");
    comes(&mut app, "point on the line");
    // It leaves for the transcript and stays there; the composer gives its place still.
    app.send_keys("C-g");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert_eq!(slots(&state), ["card"], "{state}");
    thread::sleep(LONG);
    assert_eq!(app.state()["buffer"]["name"], "*claude: proj*");
    // Gone while its line has the composer's place.
    flow.scratch.write("answers", b"gone\n");
    let state = app.wait_for("the request gone", |state| slots(state) == ["composer"]);
    assert_eq!(state["buffer"]["name"], "*claude: proj*", "{state}");
    end(&flow, app);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_press_on_the_line_as_it_took_the_composers_place_is_the_composers() {
    let (flow, mut app) = in_empty_composer("native-point-press");
    flow.scratch.write("ask", b"");
    let state = comes(&mut app, "point on the line");
    front(&mut app, false);
    let line = (state["windows"].as_array().unwrap().iter())
        .find(|window| window["docked"]["slot"] == "card")
        .unwrap();
    let at = json!({ "row": line["area"]["row"], "column": line["area"]["column"].as_u64().unwrap() + 4 });
    app.call("mouse", Some(json!({ "press": "left", "at": at })));
    app.call("mouse", Some(json!({ "release": "left", "at": at })));
    let state = app.wait_for("the composer", in_composer);
    assert_eq!(slots(&state), ["card", "composer"], "{state}");
    front(&mut app, true);
    stays(&mut app, "the composer pressed on", "");
    go(&mut app);
    app.wait_for("some of it read", |state| {
        state["request"]["may_ask"] == true
    });
    answer(&mut app, "n");
    app.wait_for("the composer once answered", in_composer);
    end(&flow, app);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_block_taller_than_its_window_shows_its_head_and_then_the_end_again() {
    let (flow, mut app) = asking("native-point-head");
    front(&mut app, true);
    japanese(&mut app, false);
    thread::sleep(LONG);
    let shown = |state: &Value| text(state);
    flow.scratch.write("ask", b"");
    // From the transcript's end, where the window follows.
    comes(&mut app, "point on the line");
    app.send_keys("C-g");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert!(
        shown(&state).starts_with("⚠ Edit asks to change a file"),
        "{state}"
    );
    assert!(!shown(&state).contains("+ new line 79"), "{state}");
    go(&mut app);
    read_on(&mut app);
    allow(&mut app);
    app.wait_for("the composer", in_composer);
    app.send_keys("C-c C-o");
    let state = app.wait_for("the transcript", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    assert!(!shown(&state).contains("asks to"), "{state}");
    assert!(shown(&state).contains("Bash"), "{state}");
    end(&flow, app);
}
