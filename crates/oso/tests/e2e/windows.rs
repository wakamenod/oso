//! Window splits in the running app (ADR 0112): `C-x 3` lays two windows side by side and
//! draws both, `C-x o` selects the other, a key goes to the selected one only and a
//! composition ends there, `C-x 0` and `C-x 1` delete windows, and a buffer that may hold
//! secrets beside the selected window keeps screenshots refused, with secure input following
//! the selected window. The wheel scrolls the window under the pointer and selects none.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The tree, the
//! commands and what each window draws have their own tests in oso-core, the dispatcher's
//! grid snapshots and the app's panes.

use serde_json::{Value, json};

use super::harness::{App, FAKE_SECRET, Scratch};
use super::png::Png;

fn windows(state: &Value) -> &[Value] {
    state["windows"].as_array().expect("windows are listed")
}

fn columns(window: &Value) -> u64 {
    window["columns"].as_u64().expect("a window's columns")
}

/// The pixels drawn at the top of the frame's right part, where only a right window's first
/// rows of text are: clear of the divider in the middle and of the modelines at the bottom.
fn drawn_at_the_top_right(shot: &Png) -> usize {
    shot.drawn_in(shot.width * 53 / 100..shot.width, 0..shot.height / 10)
}

/// The pixels drawn at the top of the frame's middle, where a divider between two halves is.
fn drawn_at_the_top_middle(shot: &Png) -> usize {
    shot.drawn_in(
        shot.width * 49 / 100..shot.width * 51 / 100,
        0..shot.height / 10,
    )
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_3_draws_both_windows_and_a_key_goes_to_the_selected_one() {
    let scratch = Scratch::new("windows-split");
    let file = scratch.write("notes.md", b"agenda\nbudget\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.visual_aids_off();
    let state = app.wait_until_ready();
    let whole = columns(&state["window"]);
    let one = app.screenshot();
    assert_eq!(drawn_at_the_top_right(&one), 0, "short lines");
    assert_eq!(drawn_at_the_top_middle(&one), 0, "no divider");

    app.send_keys("C-x 3");
    let state = app.state();
    let split = windows(&state);
    assert_eq!(split.len(), 2);
    assert_eq!(split[1]["selected"], true, "the new window is selected");
    assert_eq!(
        columns(&split[0]) + 1 + columns(&split[1]),
        whole,
        "the divider takes a column"
    );
    let two = app.screenshot();
    assert!(drawn_at_the_top_middle(&two) > 0, "the divider is drawn");
    // Only a system with a text backend draws text (ADR 0036): when Linux gets one this fails,
    // and this line should change with it.
    assert_eq!(
        drawn_at_the_top_right(&two) > 0,
        cfg!(target_os = "macos"),
        "the right window draws its text on macOS and only there"
    );

    // Back in the first window, its cursor goes down a line and types there; the new one's
    // stays.
    app.send_keys("C-x o C-n x");
    let state = app.state();
    let split = windows(&state);
    assert_eq!(split[0]["selected"], true);
    assert_eq!(state["text"]["rows"][1], "xbudget");
    assert_eq!(
        split[1]["selections"][0]["cursor"], 0,
        "the new window's cursor"
    );
    assert_eq!(split[0]["selections"][0]["cursor"], 8);

    app.send_keys("C-x 0");
    let state = app.state();
    assert_eq!(windows(&state).len(), 1);
    assert_eq!(columns(&state["window"]), whole, "it takes the frame back");

    app.send_keys("C-x 2 C-x 2 C-x 1");
    assert_eq!(windows(&app.state()).len(), 1);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_composition_ends_when_another_window_is_selected() {
    let scratch = Scratch::new("windows-ime");
    let file = scratch.write("notes.txt", b"abc\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-x 3");
    app.try_call("ime", Some(json!({ "preedit": "か" })))
        .expect("a composition");
    assert_eq!(app.state()["ime"]["composing"], true);
    app.run_command("other-window", None)
        .expect("the socket selects the other window");
    let state = app.state();
    assert_eq!(state["ime"]["composing"], false, "it ends");
    assert_eq!(windows(&state)[0]["selected"], true, "back in the first");
    assert_eq!(state["text"]["rows"][0], "abc", "nothing of it went in");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_secret_buffer_beside_keeps_screenshots_refused_and_secure_input_follows_selection() {
    let scratch = Scratch::new("windows-secret");
    let plain = scratch.write("plain.txt", b"plain text\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&plain), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-x 3");
    app.open_file(&env);
    app.wait_until_ready();
    app.send_keys("C-x o");
    let state = app.state();
    assert_eq!(
        state["buffer"]["name"], "plain.txt",
        "the plain window is selected"
    );
    let refused = app
        .try_call("screenshot", None)
        .expect_err("no screenshot with a secret beside");
    assert!(refused.contains("secret"), "{refused}");
    if cfg!(target_os = "macos") {
        let wanted = |state: &Value| state["secure_input"]["wanted"] == true;
        assert!(!wanted(&state), "keys go to the plain window");
        app.send_keys("C-x o");
        app.wait_for("secure input in the secret window", wanted);
        app.send_keys("C-x o");
        app.wait_for("none back in the plain one", |state| !wanted(state));
    }
}

fn top(window: &Value) -> u64 {
    window["scroll"]["top"]
        .as_u64()
        .expect("a window's top row")
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_wheel_scrolls_the_window_under_the_pointer_without_selecting_it() {
    let scratch = Scratch::new("windows-wheel");
    let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let file = scratch.write("long.txt", text.as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.visual_aids_off();
    app.wait_until_ready();
    app.send_keys("C-x 2");
    let state = app.state();
    let split = windows(&state);
    assert_eq!(split[1]["selected"], true, "the lower window is selected");
    let upper_rows = split[0]["rows"].as_u64().expect("a window's rows");
    // Below the upper window's text and its modeline.
    let lower_row = upper_rows + 2;

    // A notch down over the upper window scrolls it and leaves the selection alone.
    let wheel = |row: u64| json!({ "lines": [0, -1], "at": { "row": row, "column": 2 } });
    let scrolled = app.call("wheel", Some(wheel(1)));
    assert_eq!(scrolled["moved"], true);
    let state = app.state();
    let split = windows(&state);
    assert_eq!(top(&split[0]), 3, "one notch is three rows");
    assert_eq!(top(&split[1]), 0);
    assert_eq!(split[1]["selected"], true, "nothing else is selected");

    // Over the lower window, that one, by another id.
    let lower = app.call("wheel", Some(wheel(lower_row)));
    assert_ne!(lower["window"], scrolled["window"]);
    let state = app.state();
    assert_eq!((top(&windows(&state)[0]), top(&windows(&state)[1])), (3, 3));

    // In the margin, and with no place, the selected window.
    let margin = json!({ "lines": [0, -1], "at": { "x": 1.0, "y": 1.0 } });
    assert_eq!(app.call("wheel", Some(margin))["window"], lower["window"]);
    let nowhere = json!({ "lines": [0, 1] });
    assert_eq!(app.call("wheel", Some(nowhere))["window"], lower["window"]);
    let state = app.state();
    assert_eq!(top(&windows(&state)[1]), 3, "down a notch, then up one");
    assert_eq!(top(&windows(&state)[0]), 3);
}
