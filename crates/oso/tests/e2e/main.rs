//! End-to-end flows through a real dev build, over the control socket: layer 5
//! of the test pyramid (ADR 0037, ADR 0049). Run them with `just e2e`. They open
//! windows, so plain `cargo test` and `just ci` skip them.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test.
//! Each flow checks that components meet, not what they compute: motion, widths,
//! decoding and the grid have their own tests.

// The control socket supports only Unix for now (ADR 0032).
#![cfg(unix)]

mod harness;
mod png;

use std::fmt::Write as _;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use harness::{App, FAKE_SECRET, Scratch, process_exists};
use png::Foreground;
use serde_json::{Value, json};

/// A file of `lines` numbered lines, each `line NNN`.
fn numbered_lines(lines: usize) -> Vec<u8> {
    let mut text = String::new();
    for line in 0..lines {
        writeln!(text, "line {line:03}").expect("writing to a String cannot fail");
    }
    text.into_bytes()
}

/// Checks that visible row `index` of a state reads `expected`. Says which row
/// differs without quoting it.
#[track_caller]
fn assert_visible_row(state: &Value, index: usize, expected: &str) {
    let row = state["text"]["rows"][index].as_str();
    assert!(
        row == Some(expected),
        "visible row {index} is not the expected text ({:?} bytes against {})",
        row.map(str::len),
        expected.len(),
    );
}

/// The cursor's place in a state.
fn cursor(state: &Value) -> (u64, u64) {
    let cursor = &state["cursor"];
    (
        cursor["row"].as_u64().expect("a cursor has a row"),
        cursor["column"].as_u64().expect("a cursor has a column"),
    )
}

/// The first row a state's window shows.
fn scroll_top(state: &Value) -> u64 {
    state["scroll"]["top"].as_u64().expect("a scroll position")
}

/// Checks that a response line or a log does not quote the fake secret.
#[track_caller]
fn assert_no_secret(line: &str, what: &str) {
    assert!(!line.contains(FAKE_SECRET), "{what} quotes the secret");
}

/// The top-level fields that differ between two states, by name, so that a
/// message can say which without quoting their values.
fn changed_fields<'a>(before: &'a Value, after: &Value) -> Vec<&'a str> {
    let fields = before.as_object().expect("a state is an object");
    fields
        .iter()
        .filter(|(name, value)| after.get(name.as_str()) != Some(*value))
        .map(|(name, _)| name.as_str())
        .collect()
}

/// Whether this runs on CI, where no real input reaches the windows.
fn on_ci() -> bool {
    std::env::var_os("CI").is_some_and(|value| !value.is_empty())
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn keys_and_commands_reach_the_window_and_the_state_reads_them_back() {
    let scratch = Scratch::new("drive");
    let file = scratch.write("lines.txt", &numbered_lines(300));
    let mut app = App::start(Some(&file), scratch.path());

    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["path"].as_str(), file.to_str());
    assert_eq!(state["buffer"]["read_only"], Value::Null);
    assert_eq!(state["buffer"]["lines"], 301, "with the empty last line");
    assert_visible_row(&state, 0, "line 000");
    assert_visible_row(&state, 1, "line 001");
    assert_eq!(cursor(&state), (0, 0));

    // Keys, in the notation and through the path window keys take.
    app.send_keys("C-n C-n <right> C-f");
    assert_eq!(cursor(&app.state()), (2, 2));

    // A command by name, with an argument, and one that does not exist.
    app.run_command("next-line", Some(3)).unwrap();
    assert_eq!(cursor(&app.state()), (5, 2));
    let error = app.run_command("no-such-command", None).unwrap_err();
    assert!(error.contains("no-such-command"), "{error}");

    // A scroll moves the window, and the text it reports with it.
    app.send_keys("C-v");
    let state = app.state();
    let top = scroll_top(&state);
    assert!(top > 0, "a page down scrolls");
    assert_visible_row(&state, 0, &format!("line {top:03}"));

    // `oso ctl`, the way into the socket for people and agents.
    let (status, output) = app.ctl(&["version"]);
    assert!(status.success(), "{status}");
    let version: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(version["result"]["pid"], app.pid());
    let (status, _) = app.ctl(&["show-buffer", "999"]);
    assert_eq!(status.code(), Some(1), "an error response");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_screenshot_is_a_png_of_the_window_with_the_cursor_and_the_text() {
    let scratch = Scratch::new("screenshot");
    // Empty lines draw no glyphs on any system, so only the cursor shows.
    let blank = scratch.write("blank.txt", b"\n\n\n\n");
    let text = scratch.write("text.txt", &numbered_lines(5));
    let mut app = App::start(Some(&blank), scratch.path());

    let state = app.wait_until_ready();
    let shot = app.screenshot();
    assert_eq!(
        json!({ "width": shot.width, "height": shot.height }),
        state["surface"],
        "the image has the window's size",
    );
    let Some(top_cursor) = shot.foreground() else {
        panic!("the screenshot is blank");
    };
    let window_pixels = u64::from(shot.width) * u64::from(shot.height);
    assert!(
        (top_cursor.pixels as u64) < window_pixels / 4,
        "the cursor is a small part of the window: {top_cursor:?}",
    );

    // A key changes the pixels, by one row of cells.
    app.send_keys("C-n");
    let moved = app.screenshot().foreground().expect("the cursor is drawn");
    assert_eq!(
        moved,
        Foreground {
            top: top_cursor.top + top_cursor.height,
            ..top_cursor
        },
        "the cursor moved one row down",
    );

    // Only a system with a text backend draws text (ADR 0036). When Linux gets
    // one this fails, and this line should change with it.
    app.open_file(&text);
    app.wait_until_ready();
    let with_text = app.screenshot().foreground().expect("the cursor is drawn");
    assert_eq!(
        with_text.pixels > top_cursor.pixels,
        cfg!(target_os = "macos"),
        "text draws on macOS and only there: {with_text:?} against {top_cursor:?}",
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_with_invalid_bytes_opens_read_only() {
    let scratch = Scratch::new("invalid");
    let file = scratch.write("latin1.txt", b"caf\xE9\nnext\n");
    let mut app = App::start(Some(&file), scratch.path());

    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["invalid_bytes"], 1);
    assert_eq!(state["buffer"]["read_only"], "invalid-utf8");
    assert_visible_row(&state, 0, "caf\u{FFFD}");
    assert_visible_row(&state, 1, "next");
}

/// Checks that an instance showing a secret file gives away none of it.
#[track_caller]
fn assert_withheld(app: &mut App) {
    let state = app
        .exchange("get-state", None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_no_secret(&state, "get-state");
    let parsed: Value = serde_json::from_str(&state).expect("a JSON response");
    assert_eq!(parsed["result"]["buffer"]["secret"], true);
    assert!(
        parsed["result"]["text"].is_null(),
        "get-state returned text"
    );
    for (method, params) in [
        ("get-text", Some(json!({ "start_row": 0, "rows": 5 }))),
        ("screenshot", None),
    ] {
        let line = app
            .exchange(method, params)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_no_secret(&line, method);
        let response: Value = serde_json::from_str(&line).expect("a JSON response");
        let message = response["error"]["message"].as_str();
        assert!(
            message.is_some_and(|message| message.contains("ADR 0044")),
            "{method} was not refused: {message:?}",
        );
    }
    let line = app
        .exchange("export-state", None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_no_secret(&line, "export-state");
    let exported: Value = serde_json::from_str(&line).expect("a JSON response");
    let path = &parsed["result"]["buffer"]["path"];
    let buffers = exported["result"]["buffers"].as_array();
    assert!(
        buffers.is_some_and(|buffers| buffers.iter().any(|buffer| buffer["path"] == *path)),
        "export-state did not hand over the secret buffer by its path",
    );
}

/// Checks that the text and pixels of an ordinary buffer can be read.
#[track_caller]
fn assert_readable(app: &mut App, row_zero: &str) {
    let state = app.state();
    assert_eq!(state["buffer"]["secret"], false);
    assert_visible_row(&state, 0, row_zero);
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 1 })));
    assert!(
        text["rows"][0].as_str() == Some(row_zero),
        "get-text returned another row",
    );
    assert!(
        app.screenshot().foreground().is_some(),
        "a blank screenshot"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_secret_file_gives_no_text_or_pixels_until_another_buffer_is_shown() {
    let scratch = Scratch::new("secret");
    let secret = format!("{FAKE_SECRET}\n");
    let plain = scratch.write("plain.txt", b"plain text\n");
    let env = scratch.write(".env", secret.as_bytes());
    let key = scratch.write("fake-home/.ssh/id_fake", secret.as_bytes());
    // The name says nothing: only where it leads does.
    let link = scratch.symlink("notes.txt", &key);

    // By a link, at startup.
    let mut app = App::start(Some(&link), scratch.path());
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["path"].as_str(), link.to_str());
    assert_withheld(&mut app);

    // By name, with the window up, then back and forth.
    app.open_file(&plain);
    let state = app.wait_until_ready();
    let plain_id = state["buffer"]["id"].clone();
    assert_readable(&mut app, "plain text");
    app.open_file(&env);
    let state = app.wait_until_ready();
    let env_id = state["buffer"]["id"].clone();
    assert_withheld(&mut app);
    app.show_buffer(&plain_id);
    assert_readable(&mut app, "plain text");
    app.show_buffer(&env_id);
    assert_withheld(&mut app);

    let log = app.stop_and_read_log();
    assert_no_secret(&log, "the log");
    // It logs this at the level `just e2e` sets, so the log is worth checking.
    assert!(
        log.contains("listening on"),
        "no log to check; run this with `just e2e`, which sets RUST_LOG",
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_state_moves_to_a_new_instance_and_a_wrong_version_changes_nothing() {
    let scratch = Scratch::new("handoff");
    let file = scratch.write("lines.txt", &numbered_lines(300));
    let mut old = App::start(Some(&file), scratch.path());
    old.wait_until_ready();
    old.send_keys("C-v C-n C-f");
    let before = old.state();
    assert!(scroll_top(&before) > 0, "a place that needs restoring");
    let exported = old.call("export-state", None);

    // A version this build does not read is refused, and leaves it clean.
    let mut new = App::start(None, scratch.path());
    new.wait_until_ready();
    let mut wrong = exported.clone();
    wrong["version"] = json!(exported["version"].as_u64().unwrap() + 1);
    let error = new
        .try_call("import-state", Some(json!({ "state": wrong })))
        .unwrap_err();
    assert!(error.contains("version"), "{error}");
    let clean = new.state();
    assert_eq!(clean["buffers"].as_array().map(Vec::len), Some(1));
    assert_eq!(clean["buffer"]["path"], Value::Null);

    // The real state brings back the buffer and the position.
    new.call("import-state", Some(json!({ "state": exported })));
    let offset = before["cursor"]["offset"].clone();
    let after = new.wait_for("restored cursor", |state| {
        state["buffer"]["loading"].is_null() && state["cursor"]["offset"] == offset
    });
    assert_eq!(after["buffer"]["path"].as_str(), file.to_str());
    assert_eq!(cursor(&after), cursor(&before));
    assert_eq!(after["scroll"], before["scroll"]);
    assert_visible_row(&after, 0, &format!("line {:03}", scroll_top(&after)));

    // It cannot be imported on top of that, and the old instance can go.
    let exported = old.call("export-state", None);
    assert!(
        new.try_call("import-state", Some(json!({ "state": exported })))
            .is_err(),
        "the instance has opened something",
    );
    drop(old);
    assert_eq!(new.state()["cursor"]["offset"], offset);
}

/// Text no response may quote.
const UNSAVED: &str = "UNSAVED-E2E-TEXT";

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_edit_holds_the_handover_and_a_generation_guards_the_exit() {
    let scratch = Scratch::new("unsaved");
    let file = scratch.write("notes.txt", b"on disk\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let checked = app.call("check-handover", None)["generation"].clone();
    let exported = app.call("export-state", None);
    assert_eq!(exported["generation"], checked);

    app.call("insert-text", Some(json!({ "text": UNSAVED })));

    // No way of asking gives the text, and each names the buffer.
    for method in ["check-handover", "export-state"] {
        let line = app.exchange(method, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(!line.contains(UNSAVED), "{method} quotes the text");
        let response: Value = serde_json::from_str(&line).expect("a JSON response");
        let message = response["error"]["message"].as_str();
        assert!(
            message.is_some_and(|message| message.contains("notes.txt")),
            "{method} did not refuse by name: {message:?}",
        );
    }

    // The exit a supervisor makes is refused, and the instance goes on.
    let line = app
        .exchange("exit", Some(json!({ "generation": checked })))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!line.contains(UNSAVED), "exit quotes the text");
    assert!(line.contains("exit refused"), "exit was not refused");
    assert_eq!(app.state()["buffer"]["path"].as_str(), file.to_str());
    assert!(process_exists(app.pid()), "the instance is gone");

    // An ordinary `exit`, as people and agents send it, still goes.
    assert!(
        app.state()["buffer"]["bytes"].as_u64() > Some(8),
        "the edit"
    );
    let _ = app.exchange("exit", None);
    assert_eq!(app.wait_for_exit().code(), Some(0));

    // A guarded exit with nothing changed goes too.
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let current = app.call("check-handover", None)["generation"].clone();
    let _ = app.exchange("exit", Some(json!({ "generation": current })));
    assert_eq!(app.wait_for_exit().code(), Some(0));

    // Text in a new instance's `*scratch*` stops the import that would hide it.
    let mut old = App::start(Some(&file), scratch.path());
    old.wait_until_ready();
    let state = old.call("export-state", None);
    let mut new = App::start(None, scratch.path());
    new.wait_until_ready();
    new.call("insert-text", Some(json!({ "text": UNSAVED })));
    let line = new
        .exchange("import-state", Some(json!({ "state": state })))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!line.contains(UNSAVED), "import-state quotes the text");
    assert!(line.contains("*scratch*"), "import-state was not refused");
    assert_eq!(new.state()["buffers"].as_array().map(Vec::len), Some(1));

    let log = new.stop_and_read_log();
    assert!(!log.contains(UNSAVED), "the log quotes the text");
    assert!(log.contains("listening on"), "no log to check");
}

/// The event loop goes on delivering what is queued after an exit, so this can
/// only fail when the two land together, which a few tries make likely.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_edit_is_never_confirmed_and_then_lost_with_an_exit() {
    let scratch = Scratch::new("exit-race");
    for _ in 0..4 {
        let mut app = App::start(None, scratch.path());
        app.wait_until_ready();
        let generation = app.call("check-handover", None)["generation"].clone();
        let (exit, edit) = app.race(
            &json!({ "id": 1, "method": "exit", "params": { "generation": generation } }),
            &json!({ "id": 2, "method": "insert-text", "params": { "text": UNSAVED } }),
        );
        assert!(
            !(exit.contains("\"exiting\":true") && edit.contains("\"bytes\"")),
            "the instance agreed to exit and also confirmed an edit",
        );
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_quit_key_and_exit_end_the_process_with_status_zero() {
    let scratch = Scratch::new("exit");
    let ways = [
        ("send-keys", Some(json!({ "keys": "C-x C-c" }))),
        ("exit", None),
    ];
    for (name, params) in ways {
        let mut app = App::start(None, scratch.path());
        app.wait_until_ready();
        let socket = app.socket();
        assert!(socket.exists(), "the socket is there while it runs");
        // It may exit before answering.
        let _ = app.exchange(name, params);
        let status = app.wait_for_exit();
        assert_eq!(status.code(), Some(0), "after {name}: {status}");
        assert!(!socket.exists(), "{name} leaves the socket behind");
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_failing_test_leaves_nothing_behind_and_other_instances_alone() {
    let scratch = Scratch::new("cleanup");
    let file = scratch.write("lines.txt", &numbered_lines(50));
    // Stands in for an instance the user has running.
    let mut bystander = App::start(Some(&file), scratch.path());
    bystander.wait_until_ready();
    bystander.send_keys("C-n C-n");
    let before = bystander.state();
    let shot = bystander.call("screenshot", None);
    let shot = shot["path"].as_str().unwrap().to_owned();

    // A flow that fails, on a thread so that this test can look afterwards.
    let (left_behind, seen) = mpsc::channel();
    let cwd = scratch.path().to_owned();
    let opened = file.clone();
    let failed = thread::spawn(move || {
        let scratch = Scratch::new("cleanup-inner");
        let mut app = App::start(Some(&opened), &cwd);
        app.wait_until_ready();
        left_behind
            .send((app.pid(), app.socket(), scratch.path().to_owned()))
            .expect("the test is waiting");
        panic!("a deliberate failure, to test the cleanup");
    })
    .join();
    assert!(failed.is_err(), "the flow failed");

    let (inner_pid, socket, inner_scratch) = seen.recv().expect("the flow got that far");
    assert!(!process_exists(inner_pid), "the process is gone");
    assert!(!socket.exists(), "the socket is gone");
    assert!(!inner_scratch.exists(), "the scratch directory is gone");

    assert!(process_exists(bystander.pid()));
    assert!(bystander.socket().exists());
    assert!(Path::new(&shot).exists(), "its files stay");
    // Real input can move its positions and size, since its window is on the
    // screen for anyone to scroll. CI has no such input, so there nothing may
    // change. `App::start` checks that sockets lead to the right instance.
    let mut changed = changed_fields(&before, &bystander.state());
    changed.retain(|field| on_ci() || *field == "buffers");
    assert!(
        changed.is_empty(),
        "the other instance's {changed:?} changed",
    );
}
