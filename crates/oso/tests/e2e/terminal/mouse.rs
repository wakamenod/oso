//! The mouse in terminals (ADR 0120, step 4b-2): a click selects any window, and the first on
//! a terminal only selects it; a drag selects cells and the release copies them to the kill
//! ring; a program that asked for reports gets them, in SGR here, unless Shift is held; and a
//! copy while the program reads a password is a secret. Each flow runs under the shell the
//! system has as `/bin/sh` (dash on Linux CI, bash on macOS).
//!
//! Must not depend on: the app's internals; the routing, the encoder and the selection's text
//! have unit tests.

use serde_json::{Value, json};

use super::super::harness::App;
use super::{echo_off, open_terminal, rows, start, terminal, type_line, wait_for_row};

/// The selected window's state.
fn selected_window(state: &Value) -> &Value {
    state["windows"]
        .as_array()
        .and_then(|windows| windows.iter().find(|w| w["selected"] == true))
        .unwrap_or(&Value::Null)
}

/// The frame cell of the terminal's screen at `row`, `column`, in the selected window: its
/// text is the area's right part, as wide as the PTY.
fn frame_cell(state: &Value, row: u64, column: u64) -> Value {
    let area = &selected_window(state)["area"];
    let width = terminal(state)["size"]["columns"].as_u64().unwrap_or(0);
    let left = area["column"].as_u64().unwrap_or(0)
        + area["columns"].as_u64().unwrap_or(0).saturating_sub(width);
    json!({ "row": area["row"].as_u64().unwrap_or(0) + row, "column": left + column })
}

/// The screen row whose text is `wanted`, trimmed, or which ends in a prompt and then it.
fn row_of(state: &Value, wanted: &str) -> u64 {
    let after_prompt = format!("$ {wanted}");
    let found = rows(state)
        .iter()
        .position(|row| row.trim_start() == wanted || row.ends_with(&after_prompt));
    found.map_or_else(
        || panic!("no row {wanted:?}: {:?}", rows(state)),
        |i| i as u64,
    )
}

fn mouse(app: &mut App, params: Value) {
    app.call("mouse", Some(params));
}

/// Drags with the left button from `from` to `to`, frame cells, with Shift held or not.
fn drag(app: &mut App, from: Value, to: Value, shift: bool) {
    mouse(app, json!({ "press": "left", "at": from, "shift": shift }));
    mouse(app, json!({ "move": true, "at": to, "shift": shift }));
    mouse(app, json!({ "release": "left", "at": to, "shift": shift }));
}

fn newest_kill(state: &Value) -> Value {
    state["kill_ring"]["newest"]["pieces"].clone()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_click_selects_any_window_and_point_stays() {
    let (_scratch, mut app) = start("terminal-mouse-click");
    open_terminal(&mut app);
    app.send_keys("C-x 2 C-x o C-x b");
    type_line(&mut app, "*scratch*");
    let state = app.wait_for("scratch below", |state| {
        state["buffer"]["name"] == "*scratch*"
    });
    let scratch = selected_window(&state).clone();
    let terminal_window = state["windows"]
        .as_array()
        .and_then(|windows| windows.iter().find(|w| w["selected"] == false))
        .cloned()
        .expect("the terminal's window");
    let area = &terminal_window["area"];
    let in_terminal = json!({ "row": area["row"], "column": area["column"].as_u64().unwrap() + 2 });
    mouse(&mut app, json!({ "press": "left", "at": in_terminal }));
    mouse(&mut app, json!({ "release": "left", "at": in_terminal }));
    let state = app.state();
    assert_eq!(
        state["buffer"]["name"], "*terminal*",
        "the click selected it"
    );
    assert!(
        newest_kill(&state).is_null(),
        "the first click only selects"
    );
    // A click in a text window selects it, and its point does not move.
    let area = &scratch["area"];
    let in_scratch = json!({ "row": area["row"].as_u64().unwrap() + 1, "column": 8 });
    mouse(&mut app, json!({ "press": "left", "at": in_scratch }));
    mouse(&mut app, json!({ "release": "left", "at": in_scratch }));
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "*scratch*");
    assert_eq!(selected_window(&state)["selections"], scratch["selections"]);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_drag_selects_cells_and_the_release_copies_them() {
    let (_scratch, mut app) = start("terminal-mouse-drag");
    open_terminal(&mut app);
    type_line(&mut app, "echo mouse-copy-test");
    let state = wait_for_row(&mut app, "mouse-copy-test");
    let row = row_of(&state, "mouse-copy-test");
    let from = frame_cell(&state, row, 0);
    let to = frame_cell(&state, row, 14);
    drag(&mut app, from.clone(), to, false);
    let state = app.state();
    assert_eq!(newest_kill(&state), json!(["mouse-copy-test"]));
    // A click alone copies nothing more.
    mouse(&mut app, json!({ "press": "left", "at": from }));
    mouse(&mut app, json!({ "release": "left", "at": from }));
    assert_eq!(newest_kill(&app.state()), json!(["mouse-copy-test"]));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_program_that_asks_gets_sgr_reports_and_shift_selects_instead() {
    let (_scratch, mut app) = start("terminal-mouse-reports");
    open_terminal(&mut app);
    type_line(&mut app, "echo shift-copy");
    wait_for_row(&mut app, "shift-copy");
    type_line(&mut app, "printf '\\033[?1000h\\033[?1006h'; cat -v");
    let state = app.wait_for("reports on", |state| terminal(state)["mouse"] == "click");
    let at = frame_cell(&state, 5, 10);
    mouse(&mut app, json!({ "press": "left", "at": at }));
    mouse(&mut app, json!({ "release": "left", "at": at }));
    // The PTY echoes the report as `cat` reads it: ESC as `^[`.
    app.wait_for("the SGR report", |state| {
        rows(state)
            .iter()
            .any(|row| row.contains("[<0;11;6M") && row.contains("[<0;11;6m"))
    });
    assert!(
        newest_kill(&app.state()).is_null(),
        "a report copies nothing"
    );
    // With Shift held, the mouse is oso's.
    let state = app.state();
    let row = row_of(&state, "shift-copy");
    drag(
        &mut app,
        frame_cell(&state, row, 0),
        frame_cell(&state, row, 4),
        true,
    );
    assert_eq!(newest_kill(&app.state()), json!(["shift"]));
    app.send_keys("C-c C-c");
    type_line(
        &mut app,
        "printf '\\033[?1000l\\033[?1006l'; echo reports-off",
    );
    wait_for_row(&mut app, "reports-off");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_copy_while_a_password_is_read_is_a_secret() {
    let (_scratch, mut app) = start("terminal-mouse-secret");
    open_terminal(&mut app);
    type_line(
        &mut app,
        "echo visible-row; stty -echo; read x; stty echo; echo pw-done",
    );
    let state = app.wait_for("the password prompt", echo_off);
    // Rows are withheld now; the row is where it was printed, the one above the cursor.
    assert!(terminal(&state)["rows"].is_null());
    assert!(state["kill_ring"].is_null(), "withheld around a secret");
    let cursor = terminal(&state)["cursor"]["row"].as_u64().unwrap_or(1);
    let row = cursor.saturating_sub(1);
    drag(
        &mut app,
        frame_cell(&state, row, 0),
        frame_cell(&state, row, 6),
        false,
    );
    type_line(&mut app, "hunter2");
    let state = app.wait_for("the prompt's end", |state| {
        rows(state).iter().any(|row| row.trim_start() == "pw-done") && !echo_off(state)
    });
    assert!(
        newest_kill(&state).is_null(),
        "the copy is secret, so left out: {}",
        state["kill_ring"]
    );
}
