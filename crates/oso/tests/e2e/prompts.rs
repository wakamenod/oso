//! Prompts in the running app (ADR 0072, ADR 0075): a line read and gone to, `C-g`, the input
//! method off while one is open, a passphrase withheld from the socket and the log, and a
//! restart held while a prompt waits.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. What a prompt
//! reads and runs, and how its float is laid out, have their own tests.

use serde_json::json;

use super::harness::{App, FAKE_SECRET, Scratch};
use super::{assert_no_secret, cursor, numbered_lines, typing};

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_g_g_reads_a_line_in_a_float_and_goes_there() {
    let scratch = Scratch::new("prompt-goto-line");
    let file = scratch.write("lines.txt", &numbered_lines(60));
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let before = app.screenshot().foreground().expect("the text is drawn");

    app.send_keys("M-g g");
    let state = app.state();
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["kind"], "number");
    assert_eq!(prompt["prompt"], "Go to line: ");
    assert_eq!(prompt["region"]["row"], 0, "attached to the top edge");
    assert_eq!(state["ime"]["wanted"], false, "a prompt reads keys");
    assert_eq!(state["buffer"]["name"], "lines.txt", "the window beneath");
    let shown = app.screenshot().foreground().expect("the float is drawn");
    assert_ne!(shown.pixels, before.pixels, "the float changed what shows");

    app.send_keys("4 2");
    assert_eq!(app.state()["prompts"][0]["text"], "42");
    app.send_keys("RET");
    let state = app.state();
    assert_eq!(state["prompts"], json!([]));
    assert_eq!(cursor(&state), (41, 0));
    assert_eq!(state["ime"]["wanted"], true, "on again after it");

    app.send_keys("M-g g 7 C-g");
    let state = app.state();
    assert_eq!(state["prompts"], json!([]));
    assert_eq!(cursor(&state), (41, 0), "C-g runs nothing");
    assert_eq!(state["echo"], "Quit");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_exit_question_stays_on_n_and_exits_on_y() {
    let scratch = Scratch::new("prompt-exit");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys("x C-x C-c");
    let prompt = app.state()["prompts"][0].clone();
    assert_eq!(prompt["kind"], "question");
    assert_eq!(
        prompt["prompt"],
        "1 modified buffer; exit without saving? (y or n)"
    );
    app.send_keys("SPC n");
    let state = app.state();
    assert_eq!(
        state["prompts"],
        json!([]),
        "SPC answers nothing, n closes it"
    );
    assert_eq!(state["buffer"]["modified"], true);
    app.send_keys("C-x C-c");
    // It may exit before answering.
    let _ = app.exchange("send-keys", Some(json!({ "keys": "y" })));
    assert_eq!(app.wait_for_exit().code(), Some(0));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_passphrase_reaches_neither_the_socket_nor_the_log() {
    let scratch = Scratch::new("prompt-passphrase");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys("<f34>");
    app.send_keys(&typing(FAKE_SECRET));
    let state = app.state();
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["kind"], "passphrase");
    assert!(prompt["text"].is_null() && prompt["prompt"].is_null());
    assert!(state.get("echo").is_none() && state.get("modeline").is_none());
    assert_no_secret(&state.to_string(), "get-state");
    let refused = app.try_call("screenshot", None).unwrap_err();
    assert!(refused.contains("secret"), "{refused}");

    app.send_keys("RET");
    let state = app.state();
    assert_eq!(state["prompts"], json!([]));
    assert!(
        state.get("echo").is_none(),
        "what its command said is withheld"
    );
    app.send_keys("<f34> M-p");
    assert_no_secret(&app.call("get-state", None).to_string(), "get-state");
    let log = app.stop_and_read_log();
    assert_no_secret(&log, "the log");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_restart_waits_while_a_prompt_is_open() {
    let scratch = Scratch::new("prompt-handover");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys("M-g g");
    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(refused.contains("a prompt is open"), "{refused}");
    app.send_keys("C-g");
    app.call("check-handover", None);
}
