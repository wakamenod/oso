//! A session's buffer across `session-switch-mode` (ADR 0165, part 4d) in a split: every window
//! of the tree that showed the session shows it in its other mode, and once it is native again
//! its composer is docked and laid out, as a session started natively shows it (ADR 0170).
//!
//! Must not depend on: the app's internals. Driven by its control socket and the fake alone;
//! the fake is `switch`'s.

use serde_json::Value;

use super::switch::{answered, flow};
use super::{composer_docked, selected_composer, text};

/// The ids of the buffers the tree's windows show, the docked ones apart.
fn shown(state: &Value) -> Vec<Value> {
    (state["windows"].as_array().into_iter().flatten())
        .filter(|window| window["docked"].is_null())
        .map(|window| window["buffer"].clone())
        .collect()
}

/// How many of the tree's windows show buffer `id`.
fn showing(state: &Value, id: &Value) -> usize {
    shown(state).iter().filter(|shown| *shown == id).count()
}

/// The session's buffer in its TUI, when the selected window shows it in both halves: a
/// terminal named `*claude: proj*` that is not the transcript `before`.
fn terminal_in_both(state: &Value, before: &Value) -> Option<Value> {
    let selected = &state["buffer"];
    let both = selected["name"] == "*claude: proj*"
        && selected["id"] != *before
        && showing(state, &selected["id"]) == 2;
    both.then(|| selected["id"].clone())
}

/// Whether the session is native again in both halves, not in its TUI's terminal `before`:
/// its composer selected, docked under a transcript both show.
fn native_in_both(state: &Value, before: &Value) -> bool {
    let host = selected_composer(state).map(|composer| &composer["docked"]["host"]);
    composer_docked(state) && host.is_some_and(|host| host != before && showing(state, host) == 2)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_split_follows_the_session_both_ways_and_its_composer_is_docked() {
    let flow = flow("native-switch-split");
    let mut app = answered(&flow);
    app.send_keys("C-x 2");
    let state = app.wait_for("the split", |state| {
        state["buffer"]["name"] == "*claude: proj*" && showing(state, &state["buffer"]["id"]) == 2
    });
    let transcript = state["buffer"]["id"].clone();

    // To the TUI: both halves show its terminal, which took the session's name. Its idle
    // screen is taller than a half, so the flow does not wait for the tracker to judge it.
    app.run_command("session-switch-mode", None).unwrap();
    let state = app.wait_for("the terminal in both halves", |state| {
        terminal_in_both(state, &transcript).is_some()
    });
    let terminal = terminal_in_both(&state, &transcript).unwrap();

    // And back, as `/exit` in a TUI the hand-off started brings it, through the same take-back
    // as `C-c C-t`: both show the transcript, its composer docked and selected.
    flow.scratch.write("quit", b"");
    app.wait_for("the transcript in both halves", |state| {
        native_in_both(state, &terminal)
    });
    app.send_keys("C-c C-o");
    app.wait_for("the transcript taken back", |state| {
        text(state).contains("Back from the TUI")
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
