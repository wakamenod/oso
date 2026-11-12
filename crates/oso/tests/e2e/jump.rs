//! Jumps by label in the running app (ADR 0116): `M-j` reads a string and its labels show
//! once `[jump] timeout` passes, a label moves the cursor, also into another window, which it
//! selects, `M-g f` labels line starts, a restart is held while a jump is on, and a jump in a
//! buffer that may hold secrets is withheld from the socket and the log.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Which labels the
//! candidates get, where they show and what each key does have their own tests in oso-core and
//! the dispatcher's.

use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::harness::{App, FAKE_SECRET, Scratch};
use super::{assert_no_secret, cursor, typing};

fn stage(state: &Value) -> &Value {
    &state["jump"]["stage"]
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_j_shows_labels_after_the_timeout_and_a_label_jumps() {
    let scratch = Scratch::new("jump-timer");
    let file = scratch.write("notes.txt", b"xab ab\nab\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("M-j a b");
    let state = app.wait_for("the labels", |state| stage(state) == "labels");
    assert_eq!(
        state["jump"],
        json!({ "active": true, "stage": "labels", "count": 3 })
    );
    assert_eq!(state["echo"], "Jump: ab");
    assert_eq!(state["ime"]["wanted"], false, "a jump reads keys");
    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(refused.contains("a jump is under way"), "{refused}");

    // The nearest match gets the first key.
    app.send_keys("a");
    let state = app.state();
    assert!(state["jump"].is_null());
    assert_eq!(cursor(&state), (0, 1));
    assert_eq!(state["ime"]["wanted"], true, "on again after it");
    app.call("check-handover", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_label_in_the_other_window_selects_it() {
    let scratch = Scratch::new("jump-windows");
    let file = scratch.write("notes.txt", b"xab ab\nab\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-x 3");
    let state = app.state();
    assert_eq!(state["windows"][1]["selected"], true);

    // The selected window's three matches take `a s d`, the other's `f g h`.
    app.send_keys("M-j a b");
    let state = app.wait_for("the labels", |state| stage(state) == "labels");
    assert_eq!(state["jump"]["count"], 6);
    app.send_keys("f");
    let state = app.state();
    assert!(state["jump"].is_null());
    assert_eq!(state["windows"][0]["selected"], true, "the jump selects it");
    assert_eq!(cursor(&state), (0, 1));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_g_f_labels_line_starts() {
    let scratch = Scratch::new("jump-lines");
    let file = scratch.write("notes.txt", b"one\ntwo\nthree\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-e");
    app.send_keys("M-g f");
    let state = app.state();
    assert_eq!(stage(&state), "labels", "no string to wait for");
    app.send_keys("s");
    let state = app.state();
    assert!(state["jump"].is_null());
    assert_eq!(cursor(&state), (1, 0), "the next line's start");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_jump_in_a_secret_buffer_reaches_neither_the_socket_nor_the_log() {
    let scratch = Scratch::new("jump-secret");
    let env = scratch.write(".env", format!("KEY={FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&env), scratch.path());
    app.wait_until_ready();
    let secret: String = FAKE_SECRET.chars().take(12).collect();
    app.send_keys(&format!("M-j {}", typing(&secret)));
    let state = app.state();
    assert_eq!(state["jump"], json!({ "active": true }));
    assert!(state.get("echo").is_none() && state.get("cursor").is_none());
    // Past the timeout, the labels show, and get-state says no more.
    thread::sleep(Duration::from_secs(1));
    let state = app.state();
    assert_eq!(state["jump"], json!({ "active": true }), "nor its labels");
    let line = state.to_string();
    assert_no_secret(&line, "get-state");
    assert!(
        !line.contains(&secret),
        "get-state quotes the jump's string"
    );
    app.send_keys("C-g");
    assert!(app.state()["jump"].is_null());
    let log = app.stop_and_read_log();
    assert_no_secret(&log, "the log");
    assert!(!log.contains(&secret), "the log quotes the jump's string");
}
