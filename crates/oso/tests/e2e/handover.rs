//! Unsaved text through a restart (ADR 0090): the text, its region and the kill ring move
//! to a new instance in pieces, in as many as a large buffer takes, and no state, log or
//! message quotes them; a buffer that may hold secrets keeps its text and holds the restart;
//! and what `oso dev` says reaches the echo area and `*Messages*`.
//!
//! Must not depend on: the app's internals; this drives two real instances over their
//! sockets as `oso dev` does.

use serde_json::{Value, json};

use super::harness::{App, FAKE_SECRET, Scratch};

/// Text no state or log may quote.
const UNSAVED: &str = "CARRIED-E2E-TEXT";
/// The keys that type [`UNSAVED`], in the notation of `send-keys`.
const UNSAVED_KEYS: &str = "C A R R I E D - E 2 E - T E X T";

/// Hands `old`'s state to `new` as `oso dev` does: exports it, moves each item a piece at a
/// time, imports it, and exits `old` with the export's generation. Returns the state and how
/// many pieces went across.
fn hand_over(old: &mut App, new: &mut App) -> (Value, usize) {
    let state = old.call("export-state", None);
    let mut pieces = 0;
    let items = state["items"]
        .as_array()
        .expect("a version 2 state")
        .clone();
    for (item, len) in items.iter().enumerate() {
        let len = len.as_u64().expect("a length");
        let mut offset = 0;
        while offset < len {
            let at = json!({ "export": state["export"], "item": item, "offset": offset });
            let piece = old.call("export-text", Some(at.clone()));
            let mut params = at;
            params["text"] = piece["text"].clone();
            new.call("import-text", Some(params));
            offset = piece["end"].as_u64().expect("an end");
            pieces += 1;
        }
    }
    new.call("import-state", Some(json!({ "state": state })));
    let _ = old.exchange("exit", Some(json!({ "generation": state["generation"] })));
    assert_eq!(
        old.wait_for_exit().code(),
        Some(0),
        "the old instance exits"
    );
    (state, pieces)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn unsaved_text_its_region_and_its_kills_move_to_a_new_instance_and_save_there() {
    let scratch = Scratch::new("carry");
    let file = scratch.write("notes.txt", b"on disk\nsecond\n");
    let mut old = App::start(Some(&file), scratch.path());
    old.wait_until_ready();
    old.send_keys(UNSAVED_KEYS);
    // A kill of the first row, then a region over the second, shown.
    old.send_keys("C-a C-SPC C-e M-w C-n C-SPC C-f C-f");
    let before = old.state();
    assert_eq!(before["windows"][0]["mark_active"], true);
    let row = format!("{UNSAVED}on disk");
    assert_eq!(before["kill_ring"]["newest"]["pieces"], json!([row]));

    for method in ["check-handover", "export-state"] {
        let line = old.exchange(method, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(!line.contains(UNSAVED), "{method} quotes the text");
    }
    let mut new = App::start(None, scratch.path());
    new.wait_until_ready();
    let (state, _) = hand_over(&mut old, &mut new);
    assert_eq!(state["version"], 2);

    let after = new.wait_for("the carried file judged", |state| {
        state["buffer"]["secret"] == false
    });
    assert_eq!(after["buffer"]["path"].as_str(), file.to_str());
    assert_eq!(after["buffer"]["modified"], true);
    assert_eq!(after["cursor"], before["cursor"]);
    assert_eq!(
        after["windows"][0]["mark_active"], true,
        "the region is shown"
    );
    assert_eq!(after["kill_ring"]["newest"]["pieces"], json!([row]));
    let text = new.call("get-text", Some(json!({ "start_row": 0, "rows": 2 })));
    assert_eq!(text["rows"], json!([row, "second"]));

    // A save from the new instance goes to the file, with no question of a change on disk.
    new.send_keys("C-x C-s");
    new.wait_for("the save to end", |state| {
        state["buffer"]["saving"] == false && state["buffer"]["modified"] == false
    });
    let on_disk = std::fs::read_to_string(&file).expect("the file is there");
    assert_eq!(on_disk, format!("{row}\nsecond\n"));

    for log in [old.stop_and_read_log(), new.stop_and_read_log()] {
        assert!(!log.contains(UNSAVED), "a log quotes the text");
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_large_unsaved_buffer_goes_across_in_many_pieces() {
    let scratch = Scratch::new("carry-large");
    let lines: String = (0..100_000).map(|n| format!("line {n:05} é\n")).collect();
    let file = scratch.write("large.txt", lines.as_bytes());
    let mut old = App::start(Some(&file), scratch.path());
    old.wait_for("the file to load", |state| {
        state["buffer"]["loading"].is_null()
    });
    old.send_keys("x");
    let bytes = old.state()["buffer"]["bytes"].clone();
    let mut new = App::start(None, scratch.path());
    new.wait_until_ready();
    let (state, pieces) = hand_over(&mut old, &mut new);
    assert!(
        state["items"][0].as_u64() > Some(1 << 20),
        "more than a line holds"
    );
    assert!(pieces > 1, "{pieces} pieces");
    let after = new.wait_for("the carried file judged", |state| {
        state["buffer"]["secret"] == false
    });
    assert_eq!(after["buffer"]["bytes"], bytes);
    let last = new.call("get-text", Some(json!({ "start_row": 99_999, "rows": 1 })));
    assert_eq!(last["rows"], json!(["line 99999 é"]));
    let first = new.call("get-text", Some(json!({ "start_row": 0, "rows": 1 })));
    assert_eq!(first["rows"], json!(["xline 00000 é"]));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_unsaved_secret_file_holds_the_restart_by_name_and_what_oso_dev_says_is_shown() {
    let scratch = Scratch::new("carry-secret");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&env), scratch.path());
    app.wait_for("the file judged", |state| {
        state["buffer"]["loading"].is_null() && state["buffer"]["secret"] == true
    });
    app.send_keys(UNSAVED_KEYS);
    // Off the screen, so that the refusal can name it.
    app.send_keys("C-h e");
    for method in ["check-handover", "export-state"] {
        let line = app.exchange(method, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !line.contains(UNSAVED) && !line.contains(FAKE_SECRET),
            "{method}"
        );
        let response: Value = serde_json::from_str(&line).expect("a JSON response");
        let message = response["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(".env"), "{method}: {message}");
        assert!(message.contains("may hold secrets"), "{method}: {message}");
    }

    let said = "oso dev: restart held: cannot hand over .env: it has unsaved edits";
    app.call("dev-message", Some(json!({ "text": said })));
    let state = app.state();
    assert_eq!(state["echo"], said);
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 100 })));
    assert!(
        rows["rows"]
            .as_array()
            .is_some_and(|rows| rows.contains(&json!(said))),
        "*Messages* has it"
    );
    let long = "x".repeat(2048);
    assert!(
        app.try_call("dev-message", Some(json!({ "text": long })))
            .is_err()
    );

    let log = app.stop_and_read_log();
    assert!(
        !log.contains(UNSAVED) && !log.contains(FAKE_SECRET),
        "the log quotes the text"
    );
}
