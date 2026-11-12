//! Undoing a large step in the running app (ADR 0119): it goes on a slice a turn, saying how
//! far in the echo area and `get-state`'s `undoing`; keys wait and run after it in order; a
//! restart waits for it; and `C-g` cancels it, dropping the keys that waited.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Slicing, holding
//! and cancelling have their own tests.

use serde_json::json;

use super::harness::{App, Scratch};

/// Lines of `x`, each replaced by `!`: enough splices that undoing them takes many turns
/// whatever the machine, as a slice applies at most 4096.
const LINES: usize = 200_000;

/// Starts the app on a file of [`LINES`] lines of `x`, and replaces each with `y` by `!`.
fn replaced(test: &str) -> (Scratch, App) {
    let scratch = Scratch::new(test);
    let file = scratch.write("lines.txt", "x\n".repeat(LINES).as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys("M-% x RET y RET !");
    app.wait_for("`!` to end", |state| state["replace"].is_null());
    (scratch, app)
}

fn rows(app: &mut App, start_row: usize, rows: usize) -> serde_json::Value {
    app.call(
        "get-text",
        Some(json!({ "start_row": start_row, "rows": rows })),
    )["rows"]
        .clone()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn keys_and_a_restart_wait_for_a_large_undo_which_says_how_far_it_is() {
    let (_scratch, mut app) = replaced("undo-hold");
    assert_eq!(rows(&mut app, 0, 1), json!(["y"]));

    app.send_keys("C-z M-> z");
    let state = app.state();
    assert_eq!(state["undoing"]["redo"], false, "{}", state["undoing"]);
    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(
        refused.contains("an undo or a redo is under way"),
        "{refused}"
    );
    let state = app.wait_for("the progress", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Undoing… "))
    });
    assert!(state["undoing"]["done"].as_f64().is_some(), "{state}");

    app.wait_for("the undo to end", |state| state["undoing"].is_null());
    // Undone, and then the keys ran in order: `M->`, then `z` on the last row.
    assert_eq!(rows(&mut app, 0, 1), json!(["x"]));
    assert_eq!(rows(&mut app, LINES, 1), json!(["z"]));
    app.call("check-handover", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_g_cancels_a_large_undo_or_redo_and_drops_the_keys_that_waited() {
    let (_scratch, mut app) = replaced("undo-cancel");
    app.send_keys("C-z M-> z C-g");
    let state = app.state();
    assert_eq!(state["undoing"], json!(null));
    assert_eq!(state["echo"], "Undo cancelled");
    assert_eq!(rows(&mut app, 0, 1), json!(["y"]), "nothing undone");
    assert_eq!(rows(&mut app, LINES, 1), json!([""]), "no `z`");

    app.send_keys("C-z");
    app.wait_for("the undo to end", |state| state["undoing"].is_null());
    assert_eq!(rows(&mut app, 0, 1), json!(["x"]));
    app.send_keys("C-M-z C-g");
    let state = app.state();
    assert_eq!(state["echo"], "Redo cancelled");
    assert_eq!(rows(&mut app, 0, 1), json!(["x"]), "nothing redone");
}
