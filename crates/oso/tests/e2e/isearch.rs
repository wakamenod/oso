//! isearch in the running app (ADR 0082): typing a search, the count its worker sends, the
//! highlights on screen, the input method off while it reads keys, a restart held while it
//! is on, migemo's kana for a romaji string and `M-m` (ADR 0116), and a search in a buffer
//! that may hold secrets withheld from the socket and the log.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Where a search
//! goes, its count and its steps back have their own tests.

use serde_json::json;

use super::harness::{App, FAKE_SECRET, Scratch};
use super::{assert_no_secret, cursor, numbered_lines, typing};

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn get_state_says_migemo_applies_until_m_m_turns_it_off() {
    let scratch = Scratch::new("isearch-migemo");
    let file = scratch.write("kana.txt", "か カ ｶ ka\n".as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-s k a");
    let state = app.wait_for("migemo", |state| state["isearch"]["migemo"] == true);
    assert_eq!(state["ime"]["wanted"], false, "isearch reads keys");

    app.send_keys("M-m");
    app.wait_for("migemo off", |state| state["isearch"]["migemo"] == false);
    app.send_keys("RET");
    assert!(app.state()["isearch"].is_null());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_s_finds_counts_highlights_and_ends_where_it_is() {
    let scratch = Scratch::new("isearch-count");
    let file = scratch.write("lines.txt", &numbered_lines(60));
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let before = app.screenshot().foreground().expect("the text is drawn");

    app.send_keys("C-s l i n e SPC 0 0");
    let state = app.wait_for("the count", |state| !state["isearch"]["count"].is_null());
    assert_eq!(
        state["isearch"]["count"],
        json!({ "index": 1, "total": 10 })
    );
    assert_eq!(state["isearch"]["string"], "line 00");
    assert_eq!(cursor(&state), (0, 7));
    assert_eq!(state["echo"], "I-search: line 00 [1/10]");
    assert_eq!(state["ime"]["wanted"], false, "isearch reads keys");
    let shown = app.screenshot().foreground().expect("the text is drawn");
    assert_ne!(shown.pixels, before.pixels, "the matches are highlighted");

    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(refused.contains("a search is under way"), "{refused}");

    app.send_keys("C-s C-s");
    let state = app.state();
    assert_eq!(cursor(&state), (2, 7));
    assert_eq!(
        state["isearch"]["count"],
        json!({ "index": 3, "total": 10 })
    );
    app.send_keys("C-f");
    let state = app.state();
    assert!(state["isearch"].is_null(), "another key ends it");
    assert_eq!(cursor(&state), (2, 8), "and then runs");
    assert_eq!(state["cursor"]["mark"]["row"], 0, "the start is the mark");
    assert_eq!(state["ime"]["wanted"], true, "on again after it");
    app.call("check-handover", None);

    app.send_keys("C-s 0 5 9 C-g");
    let state = app.state();
    assert!(state["isearch"].is_null());
    assert_eq!(cursor(&state), (2, 8), "C-g goes back where it began");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_count_over_a_large_buffer_comes_from_the_worker() {
    let scratch = Scratch::new("isearch-large");
    let file = scratch.write("lines.txt", &numbered_lines(400_000));
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_for("the load", |state| state["buffer"]["loading"].is_null());
    app.send_keys("C-s l i n e");
    let state = app.wait_for("the count", |state| !state["isearch"]["count"].is_null());
    assert_eq!(
        state["isearch"]["count"],
        json!({ "index": 1, "total": 400_000 })
    );
    app.send_keys("RET");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_search_in_a_secret_buffer_reaches_neither_the_socket_nor_the_log() {
    let scratch = Scratch::new("isearch-secret");
    let env = scratch.write(".env", format!("KEY={FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&env), scratch.path());
    app.wait_until_ready();
    let secret: String = FAKE_SECRET.chars().take(12).collect();
    app.send_keys(&format!("C-s {}", typing(&secret)));
    let state = app.state();
    assert_eq!(state["isearch"], json!({ "active": true }));
    assert!(state.get("echo").is_none() && state.get("cursor").is_none());
    let line = state.to_string();
    assert_no_secret(&line, "get-state");
    assert!(
        !line.contains(&secret),
        "get-state quotes the search string"
    );
    app.send_keys("RET");
    app.send_keys("C-s M-p");
    assert!(!app.call("get-state", None).to_string().contains(&secret));
    let log = app.stop_and_read_log();
    assert_no_secret(&log, "the log");
    assert!(!log.contains(&secret), "the log quotes the search string");
}
