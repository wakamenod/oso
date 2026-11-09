//! Files changed on disk under a running instance (ADR 0029, ADR 0083): a buffer with no
//! unsaved edits takes the new text and keeps its cursor, one with edits keeps them and asks
//! before a save goes over the file or `revert-buffer` discards them, a file gone keeps its
//! text, and a buffer that may hold secrets reloads without its text reaching the log.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Where cursors go
//! and what undo does are in oso-core's tests.

use std::fs;
use std::path::Path;

use serde_json::Value;

use super::harness::{App, FAKE_SECRET, Scratch};
use super::{assert_no_secret, assert_visible_row, cursor, numbered_lines};

/// Replaces `path` with `bytes` as git and most editors do: a new file renamed over it.
fn replace(path: &Path, bytes: &[u8]) {
    let new = path.with_extension("new");
    fs::write(&new, bytes).expect("the scratch directory is writable");
    fs::rename(&new, path).expect("the scratch directory is writable");
}

fn modeline(state: &Value) -> &str {
    state["modeline"].as_str().unwrap_or("")
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_changed_on_disk_reloads_and_keeps_the_cursor_and_undo_takes_it_back() {
    let scratch = Scratch::new("reload-unmodified");
    let file = scratch.write("notes.txt", &numbered_lines(20));
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-n C-n C-n C-n C-n C-f C-f");
    assert_eq!(cursor(&app.state()), (5, 2));

    let changed = String::from_utf8(numbered_lines(20))
        .expect("numbered lines are ASCII")
        .replace("line 000", "the very first line");
    replace(&file, changed.as_bytes());
    // The message comes once no other read has started for a moment, so that many files
    // reloaded at once are said once: it may follow the text by up to that moment.
    let state = app.wait_for("the new text, said", |state| {
        state["text"]["rows"][0] == "the very first line" && state["echo"] == "Reloaded notes.txt"
    });
    assert_eq!(cursor(&state), (5, 2), "the cursor kept its place");
    assert_eq!(state["buffer"]["modified"], false);

    app.send_keys("C-z");
    let state = app.state();
    assert_visible_row(&state, 0, "line 000");
    assert_eq!(
        state["buffer"]["modified"], true,
        "the disk holds the new text"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_changed_under_edits_keeps_them_and_a_save_asks_first() {
    let scratch = Scratch::new("reload-modified");
    let file = scratch.write("notes.txt", b"abc\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys("x");

    replace(&file, b"from elsewhere\n");
    let state = app.wait_for("the warning", |state| {
        modeline(state).starts_with("[changed on disk]")
    });
    assert_visible_row(&state, 0, "xabc");
    assert_eq!(
        state["echo"],
        "notes.txt changed on disk; your edits are kept"
    );

    app.send_keys("C-x C-s");
    let state = app.state();
    assert_eq!(state["prompts"][0]["kind"], "question");
    app.send_keys("n");
    assert_eq!(
        fs::read(&file).expect("the file is there"),
        b"from elsewhere\n",
        "nothing went over it"
    );

    // `revert-buffer` asks, then takes the file's text.
    app.send_keys("C-x x g y");
    let state = app.wait_for("the file's text", |state| {
        state["text"]["rows"][0] == "from elsewhere"
    });
    assert_eq!(state["buffer"]["modified"], false);
    assert!(!modeline(&state).contains("[changed on disk]"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_gone_keeps_its_text_and_comes_back_when_it_does() {
    let scratch = Scratch::new("reload-gone");
    let file = scratch.write("notes.txt", b"kept\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    fs::remove_file(&file).expect("the file is there");
    let state = app.wait_for("the warning", |state| {
        modeline(state).starts_with("[deleted on disk]")
    });
    assert_visible_row(&state, 0, "kept");

    fs::write(&file, b"back\n").expect("the scratch directory is writable");
    let state = app.wait_for("the file back", |state| state["text"]["rows"][0] == "back");
    assert!(!modeline(&state).contains("[deleted on disk]"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_secret_file_reloads_without_its_text_reaching_the_log() {
    let scratch = Scratch::new("reload-secret");
    let env = scratch.write(".env", b"A=1\n");
    let notes = scratch.write("notes.txt", b"abc\n");
    let mut app = App::start(Some(&env), scratch.path());
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["secret"], true);
    // The socket says nothing of a secret buffer's text or size, and the log keeps nothing
    // but warnings while one is shown, so an ordinary file is shown when it reloads.
    app.open_file(&notes);
    app.wait_until_ready();

    replace(&env, format!("{FAKE_SECRET}\nB=2\n").as_bytes());
    let state = app.wait_for("the reload", |state| state["echo"] == "Reloaded .env");
    assert_no_secret(&state.to_string(), "get-state");
    let log = app.stop_and_read_log();
    assert_no_secret(&log, "the log");
    assert!(log.contains("Reloaded .env"), "the log names the file");
}
