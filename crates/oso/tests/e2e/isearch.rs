//! isearch in the running app (ADR 0082): typing a search, the count its worker sends, the
//! highlights on screen, the input method off while it reads keys, a restart held while it
//! is on, migemo's kana for a romaji string and `M-m` (ADR 0116), and a search in a buffer
//! that may hold secrets withheld from the socket and the log.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Where a search
//! goes, its count and its steps back have their own tests.

use serde_json::{Value, json};

use super::harness::{App, FAKE_SECRET, Scratch};
use super::png::Png;
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

/// The person chooses a Japanese input mode, or another source. Whether the input method is on.
fn choose(app: &mut App, japanese: bool) -> bool {
    app.try_call("input-source", Some(json!({ "japanese": japanese })))
        .unwrap();
    app.state()["ime"]["allowed"] == json!(true)
}

fn ime(app: &mut App, params: Value) -> Result<Value, String> {
    app.try_call("ime", Some(params))
}

/// Japanese in a search (ADR 0158): off until chosen, composed on the search's line, committed
/// into the string, and off again with the search.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn choosing_japanese_composes_into_the_search() {
    let scratch = Scratch::new("isearch-japanese");
    let file = scratch.write("notes.txt", "a 日本 日本語\n".as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.visual_aids_off();
    app.wait_until_ready();
    app.send_keys("C-s");
    assert_eq!(app.state()["ime"]["allowed"], false, "until it is chosen");
    assert!(ime(&mut app, json!({ "preedit": "に" })).is_err());
    assert!(choose(&mut app, true));
    let state = app.wait_for("nothing of the buffer is given", |state| {
        state["ime"]["excerpt"].is_null()
    });
    let top = state["ime"]["area"]["y"].as_f64().unwrap();
    let before = app.screenshot();
    ime(&mut app, json!({ "preedit": "にほん", "range": [0, 9] })).unwrap();
    let state = app.state();
    assert_eq!(state["echo"], "I-search: にほん");
    assert_eq!(state["isearch"]["string"], "", "until the commit");
    assert_eq!(cursor(&state), (0, 0));
    let shown = app.screenshot();
    assert_ne!(shown.rgba, before.rgba, "the composition is drawn");
    let first_line = |png: &Png| png.foreground().map(|found| (found.pixels, found.width));
    assert_eq!(
        first_line(&shown),
        first_line(&before),
        "in no line of the buffer"
    );
    ime(&mut app, json!({ "commit": "日本" })).unwrap();
    let state = app.wait_for("the count", |state| !state["isearch"]["count"].is_null());
    assert_eq!(state["echo"], "I-search: 日本 [1/2]");
    assert_eq!(cursor(&state), (0, 6));
    assert!(
        state["ime"]["area"]["y"].as_f64().unwrap() >= top,
        "on the echo row"
    );
    app.send_keys("C-s");
    assert_eq!(cursor(&app.state()), (0, 11));
    // A prefix key waits with it off, and 英数 turns it off.
    assert!(!choose(&mut app, false));
    assert!(choose(&mut app, true));
    app.send_keys("C-c");
    assert_eq!(app.state()["ime"]["allowed"], false);
    app.send_keys("C-g");
    assert_eq!(app.state()["ime"]["allowed"], true);
}

/// What was chosen in a search, and what was being composed, go with it (ADR 0158).
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn japanese_ends_with_its_search_and_never_starts_beside_a_secret() {
    let scratch = Scratch::new("isearch-japanese-ends");
    let file = scratch.write("notes.txt", "a 日本 日本語\n".as_bytes());
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    let notes = app.wait_until_ready()["buffer"]["id"].clone();
    app.send_keys("C-s");
    assert!(choose(&mut app, true));
    // A composition ends with the search it was made in, in no buffer.
    ime(&mut app, json!({ "preedit": "ご" })).unwrap();
    app.send_keys("RET");
    let state = app.state();
    assert!(state["isearch"].is_null());
    assert_eq!(state["ime"]["composing"], false);
    assert_eq!(state["buffer"]["modified"], false);
    assert_eq!(state["ime"]["allowed"], true, "the buffer's own");
    // One cancelled in a search takes no later one with it, however that search ended.
    app.send_keys("C-s");
    assert!(choose(&mut app, true));
    ime(&mut app, json!({ "preedit": "ご" })).unwrap();
    ime(&mut app, json!({ "clear": true })).unwrap();
    app.send_keys("RET");
    ime(&mut app, json!({ "preedit": "か" })).unwrap();
    assert_eq!(app.state()["ime"]["preedit"], "か");
    ime(&mut app, json!({ "clear": true })).unwrap();
    // The next search starts without, and so does a prompt that ends one.
    app.send_keys("C-s");
    assert_eq!(app.state()["ime"]["allowed"], false);
    assert!(choose(&mut app, true));
    app.send_keys("M-x");
    let state = app.state();
    assert!(state["isearch"].is_null());
    assert_eq!(state["ime"]["allowed"], false);
    app.send_keys("C-g");

    // Never where a buffer that may hold secrets is searched, or shown beside.
    app.open_file(&env);
    app.wait_until_ready();
    app.send_keys("C-s");
    assert!(!choose(&mut app, true));
    app.send_keys("C-g C-x 2 C-x o");
    app.show_buffer(&notes);
    app.send_keys("C-s");
    assert!(!choose(&mut app, true), "one is in view");
    app.send_keys("C-g C-x 1 C-s");
    assert!(choose(&mut app, true));
}
