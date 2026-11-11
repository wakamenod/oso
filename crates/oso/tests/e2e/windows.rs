//! Window splits in the running app (ADR 0112): `C-x 3` lays two windows side by side and
//! draws both, `C-x o` selects the other, a key goes to the selected one only, and `C-x 0` and
//! `C-x 1` delete windows.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The tree, the
//! commands and what each window draws have their own tests in oso-core, the dispatcher's
//! grid snapshots and the app's panes.

use serde_json::Value;

use super::harness::{App, Scratch};

fn windows(state: &Value) -> &[Value] {
    state["windows"].as_array().expect("windows are listed")
}

fn columns(window: &Value) -> u64 {
    window["columns"].as_u64().expect("a window's columns")
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
    let before = app.screenshot();
    let one = before.foreground().expect("the text is drawn");
    assert!(one.left + one.width < before.width / 2, "at the left only");

    app.send_keys("C-x 3");
    let state = app.state();
    let split = windows(&state);
    assert_eq!(split.len(), 2);
    assert_eq!(split[0]["selected"], true, "the original stays selected");
    assert_eq!(
        columns(&split[0]) + 1 + columns(&split[1]),
        whole,
        "the divider takes a column"
    );
    let shot = app.screenshot();
    let two = shot.foreground().expect("both windows' text is drawn");
    assert!(
        two.left + two.width > shot.width / 2,
        "the right window draws its text too"
    );

    app.send_keys("C-x o x");
    let state = app.state();
    assert_eq!(windows(&state)[1]["selected"], true);
    assert_eq!(
        state["text"]["rows"][0], "xagenda",
        "typed in the selected window"
    );

    app.send_keys("C-x 0");
    let state = app.state();
    assert_eq!(windows(&state).len(), 1);
    assert_eq!(columns(&state["window"]), whole, "it takes the frame back");

    app.send_keys("C-x 2 C-x 2 C-x 1");
    assert_eq!(windows(&app.state()).len(), 1);
}
