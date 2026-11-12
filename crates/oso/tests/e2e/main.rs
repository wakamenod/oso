//! End-to-end flows through a real dev build, over the control socket: layer 5
//! of the test pyramid (ADR 0037, ADR 0049). Run them with `just e2e`. They open
//! windows, so plain `cargo test` and `just ci` skip them.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test.
//! Each flow checks that components meet, not what they compute: motion, widths,
//! decoding and the grid have their own tests.

// The control socket supports only Unix for now (ADR 0032).
#![cfg(unix)]

mod agent;
mod buffers;
mod complete;
mod config;
mod cursors;
mod editorconfig;
mod files;
mod handover;
mod harness;
mod highlight;
mod ime;
mod indent;
mod isearch;
mod jump;
mod kill;
mod macros;
mod mark;
mod png;
mod projects;
mod prompts;
mod recovery;
mod reload;
mod replace;
#[cfg(target_os = "macos")]
mod secure;
mod windows;
mod wrap;
mod yank_pop;

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use harness::{App, FAKE_SECRET, Scratch, emergency_dir, process_exists};
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

/// The fields that differ between two states, by name, an object's own fields by
/// `name.field`, so that a message can say which without quoting their values.
fn changed_fields(before: &Value, after: &Value) -> Vec<String> {
    let fields = before.as_object().expect("a state is an object");
    let mut changed = Vec::new();
    for (name, value) in fields {
        let now = after.get(name.as_str());
        if now == Some(value) {
            continue;
        }
        // An object's own fields, one level down, so that a failure says which one moved.
        match (value.as_object(), now.and_then(Value::as_object)) {
            (Some(was), Some(is)) => changed.extend(
                was.iter()
                    .filter(|(field, inner)| is.get(field.as_str()) != Some(*inner))
                    .map(|(field, _)| format!("{name}.{field}")),
            ),
            _ => changed.push(name.clone()),
        }
    }
    changed
}

/// Whether this runs on CI, where no real input reaches the windows.
fn on_ci() -> bool {
    std::env::var_os("CI").is_some_and(|value| !value.is_empty())
}

/// With no window system to reach, the instance says why and exits with status 1, without a
/// panic, whose message would be dropped (ADR 0028).
#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn with_no_window_system_it_says_so_and_exits_with_status_one() {
    let scratch = harness::Scratch::new("no-display");
    let (status, log) = harness::start_without_a_window_system(scratch.path());
    assert_eq!(status.code(), Some(1), "{log}");
    assert!(log.contains("cannot reach the window system"), "{log}");
    assert!(!log.contains("panicked"), "{log}");
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
    app.visual_aids_off();

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

/// Checks that an instance showing a secret file gives away none of it, and hands nothing
/// over while it shows one.
#[track_caller]
fn assert_withheld(app: &mut App) {
    let state = assert_text_withheld(app);
    let buffer = &state["result"]["buffer"];
    for field in ["modified", "saving", "bytes", "lines", "invalid_bytes"] {
        assert!(buffer[field].is_null(), "get-state gave its {field}");
    }
    assert_refused_while_shown(app);
}

/// Checks that, with another buffer shown, `export-state` hands a secret buffer over by its
/// path, quoting none of it, and with no window on it to give a position.
#[track_caller]
fn assert_handed_over_unshown(app: &mut App, path: &Value) {
    let line = app
        .exchange("export-state", None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_no_secret(&line, "export-state");
    let exported: Value = serde_json::from_str(&line).expect("a JSON response");
    let buffers = exported["result"]["buffers"].as_array().expect("buffers");
    let index = buffers.iter().position(|buffer| buffer["path"] == *path);
    let index = index.expect("export-state did not hand over the secret buffer by its path");
    let windows = exported["result"]["windows"].as_array().expect("windows");
    let id = &buffers[index]["id"];
    assert!(
        windows.iter().all(|window| window["buffer"] != *id),
        "export-state put a window on the secret buffer",
    );
}

/// Checks that, with another buffer shown, a restart is refused for a modified secret buffer by
/// its name, quoting none of it.
#[track_caller]
fn assert_refused_by_name(app: &mut App, name: &str) {
    for method in ["check-handover", "export-state"] {
        let line = app.exchange(method, None).unwrap_or_else(|e| panic!("{e}"));
        assert_no_secret(&line, method);
        let response: Value = serde_json::from_str(&line).expect("a JSON response");
        let message = response["error"]["message"].as_str();
        assert!(
            message.is_some_and(|message| message.contains(name)),
            "{method} did not refuse by name: {message:?}",
        );
    }
}

/// Checks that a restart is refused for the one reason a shown secret buffer gives, which
/// says nothing of that buffer.
#[track_caller]
fn assert_refused_while_shown(app: &mut App) {
    for method in ["check-handover", "export-state"] {
        let line = app.exchange(method, None).unwrap_or_else(|e| panic!("{e}"));
        assert_no_secret(&line, method);
        let response: Value = serde_json::from_str(&line).expect("a JSON response");
        let message = response["error"]["message"].as_str();
        assert_eq!(
            message,
            Some("cannot hand over while a buffer that may hold secrets is shown"),
            "{method}",
        );
    }
}

/// Checks that no request gives the shown buffer's text or pixels. Returns the
/// response to `get-state`.
#[track_caller]
fn assert_text_withheld(app: &mut App) -> Value {
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
    for hidden in ["cursor", "scroll", "modeline", "echo"] {
        assert!(
            parsed["result"].get(hidden).is_none(),
            "get-state returned the {hidden}"
        );
    }
    for hidden in ["scroll", "primary", "selections"] {
        assert!(
            parsed["result"]["windows"][0].get(hidden).is_none(),
            "get-state returned the window's {hidden}"
        );
    }
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
    parsed
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

/// Checks that the log of the secret flow quotes no secret, no typed key and no failure.
#[track_caller]
fn assert_log_says_nothing_of_it(log: &str) {
    assert_no_secret(log, "the log");
    assert!(!log.contains('Ж'), "the log quotes a typed key");
    assert!(
        !log.contains("Beginning of buffer"),
        "the log holds what a command said around a secret buffer"
    );
    // It logs this at the level `just e2e` sets, so the log is worth checking.
    assert!(
        log.contains("listening on"),
        "no log to check; run this with `just e2e`, which sets RUST_LOG",
    );
}

/// Checks that what a failure says of where a secret buffer's cursor is stays out of
/// `*Messages*`, and out of the echo area once the buffer is out of view for `plain`.
#[track_caller]
fn assert_failure_stays_on_the_screen(app: &mut App, plain: &Value) {
    app.send_keys("M-< C-b");
    assert_text_withheld(app);
    app.show_buffer(plain);
    let echo = app.state()["echo"].clone();
    assert_eq!(echo, "", "the echo area kept a secret's failure");
    app.run_command("view-echo-area-messages", None)
        .expect("the command runs");
    let messages = app.state();
    assert_eq!(messages["buffer"]["name"], "*Messages*");
    let rows = messages["text"]["rows"].to_string();
    assert!(!rows.contains("Beginning of buffer"), "*Messages*: {rows}");
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
    let env_path = state["buffer"]["path"].clone();
    assert_withheld(&mut app);
    app.show_buffer(&plain_id);
    assert_readable(&mut app, "plain text");
    assert_handed_over_unshown(&mut app, &env_path);
    app.show_buffer(&env_id);
    assert_withheld(&mut app);
    // A motion gives no position away, by either way of asking.
    app.send_keys("M-f");
    assert_withheld(&mut app);

    // Keys leave no timing while it is shown, and do again once it is not.
    app.call("take-timings", None);
    app.send_keys("C-f");
    let timings = app.call("take-timings", None);
    assert_eq!(timings["keys_waiting"], 0, "a key waits to be timed");
    assert_eq!(timings["key_latency_us"], json!([]), "a key was timed");
    // Nor do frames count. This checks for something not happening, so it waits for
    // the key's frame by time, and only a frame that should not count can fail it.
    let frames = timings["frames"].clone();
    app.send_keys("C-f");
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        app.call("take-timings", None)["frames"],
        frames,
        "a frame was counted"
    );
    // The window was drawing all along: the next buffer's frame counts.
    app.show_buffer(&plain_id);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.call("take-timings", None)["frames"] == frames {
        assert!(Instant::now() < deadline, "the window draws no frames");
        thread::sleep(Duration::from_millis(10));
    }
    app.send_keys("C-f");
    let timings = app.call("take-timings", None);
    let samples = timings["key_latency_us"].as_array().map_or(0, Vec::len);
    assert_eq!(
        timings["keys_waiting"].as_u64().unwrap_or(0) + samples as u64,
        1,
        "a key in an ordinary buffer was not timed",
    );
    app.show_buffer(&env_id);

    // An edit does not make it readable, and a restart is refused as before it.
    app.send_keys("a");
    assert_withheld(&mut app);
    // Once it is off the screen, the edit holds a restart, by name.
    app.show_buffer(&plain_id);
    assert_refused_by_name(&mut app, ".env");
    app.show_buffer(&env_id);
    // A character typed by mistake after a prefix stays out of the log.
    app.send_keys("C-x Ж");
    assert_failure_stays_on_the_screen(&mut app, &plain_id);

    assert_log_says_nothing_of_it(&app.stop_and_read_log());
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
/// The keys that type [`UNSAVED`], in the notation of `send-keys`.
const UNSAVED_KEYS: &str = "U N S A V E D - E 2 E - T E X T";

/// How many frames the instance has counted so far.
fn frames_drawn(app: &mut App) -> u64 {
    let timings = app.call("take-timings", None);
    timings["frames"].as_u64().expect("a count of frames")
}

/// Sends `keys`, which change the echo area and nothing else, and so ask for no frame
/// themselves. Checks what it says and that a frame comes.
#[track_caller]
fn assert_echo_gets_a_frame(app: &mut App, keys: &str, echo: &str) {
    let before = frames_drawn(app);
    app.send_keys(keys);
    assert_eq!(app.state()["echo"], echo, "after {keys}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while frames_drawn(app) == before {
        assert!(Instant::now() < deadline, "no frame after {keys}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_modeline_and_the_echo_area_say_what_the_editor_does_and_get_a_frame() {
    let scratch = Scratch::new("status");
    let file = scratch.write("notes.txt", "a\u{202e}b\n".as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    let state = app.wait_until_ready();
    let modeline = state["modeline"].as_str().expect("a modeline");
    assert!(
        modeline.starts_with("[bidi controls] -- notes.txt  1:0  "),
        "{modeline}"
    );

    assert_echo_gets_a_frame(&mut app, "C-b", "Beginning of buffer");
    assert_echo_gets_a_frame(&mut app, "C-x", "C-x-");
    assert_echo_gets_a_frame(&mut app, "C-z", "C-x C-z is undefined");

    // The failure is in *Messages*, which holds no restart and cannot be typed in.
    let held = app.call("check-handover", None);
    assert!(held["generation"].is_u64(), "*Messages* held the restart");
    app.send_keys("C-h e");
    let state = app.wait_for("*Messages*", |state| {
        state["buffer"]["name"] == "*Messages*"
    });
    assert_visible_row(&state, 0, "Beginning of buffer");
    assert_eq!(state["buffer"]["read_only"], "internal");
    app.send_keys("a");
    assert_eq!(app.state()["echo"], "Buffer is read-only");
    assert!(
        app.call("export-state", None)["buffers"]
            .to_string()
            .contains("notes.txt"),
        "the file is handed over from *Messages* too"
    );
    app.send_keys("C-h e");
    assert_eq!(app.state()["buffer"]["name"], "notes.txt");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn typed_keys_reach_the_text_and_change_the_generation_until_undone() {
    let scratch = Scratch::new("typing");
    let file = scratch.write("notes.txt", b"on disk\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let clean = app.call("check-handover", None)["generation"].clone();

    app.send_keys("a 語 RET");
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 2 })));
    assert_eq!(text["rows"], json!(["a語", "on disk"]));
    let edited = app.call("check-handover", None)["generation"].clone();
    assert_ne!(edited, clean, "the edit is text only this instance holds");

    app.send_keys("C-z C-z");
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 1 })));
    assert_eq!(text["rows"], json!(["on disk"]));
    assert_eq!(app.call("check-handover", None)["generation"], clean);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_edit_changes_the_generation_and_a_generation_guards_the_exit() {
    let scratch = Scratch::new("unsaved");
    let file = scratch.write("notes.txt", b"on disk\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let checked = app.call("check-handover", None)["generation"].clone();
    let exported = app.call("export-state", None);
    assert_eq!(exported["generation"], checked);

    app.send_keys(UNSAVED_KEYS);

    // Neither way of asking gives the text: the state names its length only.
    for method in ["check-handover", "export-state"] {
        let line = app.exchange(method, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(!line.contains(UNSAVED), "{method} quotes the text");
        let response: Value = serde_json::from_str(&line).expect("a JSON response");
        assert_ne!(response["result"]["generation"], checked, "{method}");
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
    new.send_keys(UNSAVED_KEYS);
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

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_save_reaches_the_disk_and_a_restart_reads_the_file_again() {
    let scratch = Scratch::new("save");
    let file = scratch.write("notes.txt", b"on disk\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys(UNSAVED_KEYS);
    app.send_keys("C-x C-s");
    app.wait_for("the save to end", |state| {
        state["buffer"]["saving"] == false && state["buffer"]["modified"] == false
    });

    // Read from outside the editor, as anything else on the machine would.
    let on_disk = std::fs::read_to_string(&file).expect("the file is there");
    assert_eq!(on_disk, format!("{UNSAVED}on disk\n"));
    let names = std::fs::read_dir(scratch.path())
        .expect("the directory")
        .count();
    assert_eq!(names, 1, "nothing is left beside the file");

    // Saved, nothing is carried, and the new instance reads the file.
    let generation = app.call("check-handover", None)["generation"].clone();
    let state = app.call("export-state", None);
    assert_eq!(state["items"], json!([]), "the saved text is not carried");
    let mut new = App::start(None, scratch.path());
    new.wait_until_ready();
    new.call("import-state", Some(json!({ "state": state })));
    let _ = app.exchange("exit", Some(json!({ "generation": generation })));
    assert_eq!(app.wait_for_exit().code(), Some(0));
    new.wait_for("the file to load", |state| {
        state["buffer"]["loading"] == Value::Null
    });
    let text = new.call("get-text", Some(json!({ "start_row": 0, "rows": 1 })));
    assert_eq!(text["rows"], json!([format!("{UNSAVED}on disk")]));

    let log = new.stop_and_read_log();
    assert!(!log.contains(UNSAVED), "the log quotes the text");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_quit_key_asks_with_unsaved_text_and_a_save_needs_no_answer() {
    let scratch = Scratch::new("exit-guard");
    let file = scratch.write("notes.txt", b"on disk\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys(UNSAVED_KEYS);

    app.send_keys("C-x C-c");
    assert!(process_exists(app.pid()), "it asks first");
    let state = app.state();
    assert_eq!(state["buffer"]["modified"], true);
    assert_eq!(state["prompts"][0]["kind"], "question");

    // It may exit before answering.
    let _ = app.exchange("send-keys", Some(json!({ "keys": "y" })));
    assert_eq!(app.wait_for_exit().code(), Some(0));
    assert_eq!(
        std::fs::read(&file).expect("the file is there"),
        b"on disk\n",
        "confirming leaves the file as it was",
    );

    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.send_keys(UNSAVED_KEYS);
    app.send_keys("C-x C-s");
    app.wait_for("the save to end", |state| {
        state["buffer"]["modified"] == false
    });
    let _ = app.exchange("send-keys", Some(json!({ "keys": "C-x C-c" })));
    assert_eq!(app.wait_for_exit().code(), Some(0));

    // A save asked for just before a confirmed exit still lands, and whole.
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let _ = app.exchange("send-keys", Some(json!({ "keys": "x C-x C-s C-x C-c y" })));
    assert_eq!(app.wait_for_exit().code(), Some(0));
    let on_disk = std::fs::read_to_string(&file).expect("the file is there");
    assert!(on_disk.starts_with('x'), "the save did not land");
    let names = std::fs::read_dir(scratch.path())
        .expect("the directory")
        .count();
    assert_eq!(names, 1, "nothing is left beside the file");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_failed_save_after_a_confirmed_exit_keeps_the_instance_and_the_text() {
    let scratch = Scratch::new("exit-failed-save");
    let file = scratch.write("notes.txt", b"on disk\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    std::fs::write(&file, b"another program\n").expect("the file changes on disk");
    // One batch on the UI thread, so the exit is confirmed before any report arrives.
    let keys = format!("{UNSAVED_KEYS} C-x C-s C-x C-c y");
    app.send_keys(&keys);
    let state = loop {
        match app.try_call("get-state", None) {
            Ok(state) if state["buffer"]["saving"] == false => break state,
            Ok(_) => {}
            Err(error) => assert!(error.contains("shutting down"), "{error}"),
        }
        assert!(process_exists(app.pid()), "the instance went with the text");
    };
    assert_eq!(state["buffer"]["modified"], true);
    assert_eq!(
        std::fs::read(&file).expect("the file is there"),
        b"another program\n",
        "the other program's text stays",
    );
    let names = std::fs::read_dir(scratch.path())
        .expect("the directory")
        .count();
    assert_eq!(names, 1, "nothing is left beside the file");

    // The exit was taken back, so nobody has agreed to lose this any more.
    app.call("dev-end-unasked", None);
    assert!(app.wait_for_exit().success());
    let copies = read_copies(&emergency_dir(scratch.path()));
    assert_eq!(copies.len(), 1, "an end after that writes the text");
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
            &json!({ "id": 2, "method": "send-keys", "params": { "keys": UNSAVED_KEYS } }),
        );
        assert!(
            !(exit.contains("\"exiting\":true") && edit.contains("\"keys\"")),
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

/// Text typed into buffers, which no output may quote.
const FAKE_TEXT: &str = "FAKETYPEDTEXT";

/// The keys that type `text`.
fn typing(text: &str) -> String {
    let keys: Vec<String> = text.chars().map(String::from).collect();
    keys.join(" ")
}

/// The copies in `directory`: each `.meta` file's content with its text. Nothing if the
/// directory does not exist. Checks what no flow may break: a private directory, private
/// files, and a text beside every metadata file.
fn read_copies(directory: &Path) -> Vec<(String, Vec<u8>)> {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mode = |path: &Path| fs::symlink_metadata(path).expect("cannot look").mode() & 0o7777;
    assert_eq!(mode(directory), 0o700, "the directory is private");
    let paths: Vec<_> = entries
        .map(|entry| entry.expect("an entry").path())
        .collect();
    let mut copies = Vec::new();
    for path in &paths {
        assert_eq!(mode(path), 0o600, "a copy is private");
        if path
            .extension()
            .is_some_and(|extension| extension == "meta")
        {
            let meta = fs::read_to_string(path).expect("metadata is ASCII");
            let text = fs::read(path.with_extension("txt")).expect("a copy has its text");
            copies.push((meta, text));
        }
    }
    assert_eq!(
        paths.len(),
        copies.len() * 2,
        "a text and a metadata file each, and no more"
    );
    copies
}

/// Checks that nothing in `directory` holds `text`.
#[track_caller]
fn assert_nowhere_in(directory: &Path, text: &str) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let bytes = fs::read(entry.expect("an entry").path()).expect("cannot read a copy");
        assert!(
            !String::from_utf8_lossy(&bytes).contains(text),
            "a copy holds what it should not"
        );
    }
}

/// The copy of the buffer whose metadata has `line`.
#[track_caller]
fn copy_with<'a>(copies: &'a [(String, Vec<u8>)], line: &str) -> &'a [u8] {
    copies
        .iter()
        .find(|(meta, _)| meta.lines().any(|found| found == line))
        .map_or_else(|| panic!("no copy with {line:?}"), |(_, text)| text)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_panic_outside_a_command_writes_the_copies_and_quotes_no_text() {
    let scratch = Scratch::new("panic");
    let notes = scratch.write("notes.txt", b"on disk");
    let secret = scratch.write(".env", FAKE_SECRET.as_bytes());
    let mut app = App::start(Some(&notes), scratch.path());
    let state = app.wait_until_ready();
    let scratch_id = state["buffers"]
        .as_array()
        .and_then(|buffers| buffers.iter().find(|buffer| buffer["name"] == "*scratch*"))
        .expect("a *scratch* buffer")["id"]
        .clone();
    // At the front of the file, so that the bytes after it stay as they were: no final
    // newline is added.
    app.send_keys(&typing(FAKE_TEXT));
    app.show_buffer(&scratch_id);
    app.send_keys(&format!("{} SPC y", typing(FAKE_TEXT)));
    app.open_file(&secret);
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    assert_eq!(app.state()["buffer"]["secret"], true);

    // The secret buffer is shown, so the panic's message quotes the secret.
    let _ = app.exchange("dev-panic", None);
    let status = app.wait_for_exit();
    assert!(!status.success(), "a panic is a failure");

    let directory = emergency_dir(scratch.path());
    let copies = read_copies(&directory);
    assert_eq!(
        copies.len(),
        2,
        "the file and *scratch*, and not the secret"
    );
    assert_eq!(
        copy_with(&copies, &format!("path {}", encoded(&notes))),
        format!("{FAKE_TEXT}on disk").as_bytes(),
        "the text exactly, with no final newline",
    );
    assert_eq!(
        copy_with(&copies, "name *scratch*"),
        format!("{FAKE_TEXT} y").as_bytes(),
    );
    assert_nowhere_in(&directory, FAKE_SECRET);

    let log = app.stop_and_read_log();
    assert_no_secret(&log, "the log");
    assert!(!log.contains(FAKE_TEXT), "the log quotes the text");
    assert!(log.contains("oso panicked at"), "it says where");
    assert!(log.contains("control.rs"), "it says where");
    assert!(
        log.contains("wrote 2 emergency copies in"),
        "it says how many"
    );
    assert!(
        log.contains(directory.to_str().expect("UTF-8")),
        "and where"
    );
    assert!(
        log.contains("no copy of 1 unsaved buffer that may hold secrets"),
        "and what it left out"
    );
}

/// A path as metadata writes it, for scratch paths, which hold only these characters.
fn encoded(path: &Path) -> String {
    let path = path.to_str().expect("scratch paths are UTF-8");
    assert!(
        !path.contains(|c: char| !c.is_ascii_alphanumeric() && !"/._-".contains(c)),
        "a scratch path that needs encoding"
    );
    path.to_owned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_panicking_command_is_caught_writes_nothing_and_the_editor_runs_on() {
    let scratch = Scratch::new("caught");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));

    let error = app.run_command("dev-panic", None).unwrap_err();
    assert!(error.contains("panicked"), "{error}");
    app.send_keys("x");
    assert_eq!(app.state()["buffer"]["modified"], true, "it still runs");
    assert!(
        read_copies(&emergency_dir(scratch.path())).is_empty(),
        "a caught panic writes nothing",
    );

    let log = app.stop_and_read_log();
    assert!(log.contains("a command panicked at"), "it says where");
    assert!(log.contains("dev_tools.rs"), "it says where");
    assert!(!log.contains(FAKE_TEXT), "the log quotes the text");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_end_nobody_agreed_to_writes_copies_and_a_confirmed_exit_writes_nothing() {
    let scratch = Scratch::new("end");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    app.send_keys("C-x C-c");
    assert_eq!(app.state()["buffer"]["modified"], true, "it asks first");
    let _ = app.exchange("send-keys", Some(json!({ "keys": "y" })));
    assert!(app.wait_for_exit().success());
    assert!(
        read_copies(&emergency_dir(scratch.path())).is_empty(),
        "a person who confirmed has agreed to lose it",
    );
    drop(app);

    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    let _ = app.exchange("exit", None);
    assert!(app.wait_for_exit().success());
    assert!(
        read_copies(&emergency_dir(scratch.path())).is_empty(),
        "and so has the socket's `exit`",
    );
    drop(app);

    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    app.call("dev-end-unasked", None);
    assert!(app.wait_for_exit().success());
    let copies = read_copies(&emergency_dir(scratch.path()));
    assert_eq!(copies.len(), 1, "nobody agreed to this one");
    assert_eq!(copy_with(&copies, "why exit"), FAKE_TEXT.as_bytes());
    let log = app.stop_and_read_log();
    assert!(log.contains("wrote 1 emergency copy in"), "it says so");
    assert!(!log.contains(FAKE_TEXT), "the log quotes the text");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_refused_directory_is_said_at_startup_and_a_panic_then_writes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("refused");
    let directory = emergency_dir(scratch.path());
    fs::create_dir_all(&directory).expect("cannot make the directory");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
        .expect("cannot loosen the directory");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    let _ = app.exchange("dev-panic", None);
    assert!(!app.wait_for_exit().success());
    assert_eq!(
        fs::read_dir(&directory).expect("it is still there").count(),
        0,
        "nothing goes in a directory that is not safe",
    );
    let log = app.stop_and_read_log();
    assert!(
        log.contains("No emergency copy can be written if oso crashes"),
        "it said so at startup",
    );
    assert!(
        log.contains("could not write 1 emergency copy"),
        "and at the crash"
    );
    assert!(!log.contains(FAKE_TEXT), "the log quotes the text");
}

/// What the end of the session sends: a quit Apple event with a logout reason, which oso
/// never refuses (ADR 0084), then a real `applicationWillTerminate`, after which copies of
/// what is at risk are written.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_system_asking_the_app_to_quit_writes_the_copies() {
    let scratch = Scratch::new("quit");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    // Only the end of the session goes past a modified buffer unasked; a quit with no reason
    // is a person's, and asks first (ADR 0084).
    app.ask_to_quit_for_logout()
        .unwrap_or_else(|said| panic!("osascript could not ask: {said}"));
    assert!(app.wait_for_exit().success());
    let copies = read_copies(&emergency_dir(scratch.path()));
    assert_eq!(copies.len(), 1);
    assert_eq!(copy_with(&copies, "why exit"), FAKE_TEXT.as_bytes());
}

/// A Dock quit, or another app's with no reason, is guarded as `C-x C-c` is on oso's own layer
/// (ADR 0084): the exit question opens, `n` stays, and `y` goes with nothing written, as the
/// person agreed.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_persons_quit_asks_the_exit_question() {
    let scratch = Scratch::new("quit-asks");
    let mut app = App::start(None, scratch.path());
    app.wait_until_ready();
    app.send_keys(&typing(FAKE_TEXT));
    let question = |state: &Value| {
        state["prompts"][0]["prompt"] == "1 modified buffer; exit without saving? (y or n)"
    };
    app.ask_to_quit()
        .unwrap_or_else(|said| panic!("osascript could not ask: {said}"));
    app.wait_for("the exit question", question);
    app.send_keys("n");
    let state = app.wait_for("the question answered", |state| {
        state["prompts"] == json!([])
    });
    assert_eq!(state["buffer"]["modified"], true, "still here");
    app.ask_to_quit()
        .unwrap_or_else(|said| panic!("osascript could not ask: {said}"));
    app.wait_for("the exit question again", question);
    // It may exit before answering.
    let _ = app.exchange("send-keys", Some(json!({ "keys": "y" })));
    assert_eq!(app.wait_for_exit().code(), Some(0));
    assert!(
        read_copies(&emergency_dir(scratch.path())).is_empty(),
        "agreed: none written"
    );
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
    // The input method is told the cursor's cell only once a frame has been presented, which
    // a slow runner's first instance may not have reached yet. The next event would give it,
    // as the inner instance's window takes and gives back the focus, so the state compared
    // is the one that has it.
    let before = bystander.wait_for("the cursor's cell for the input method", |state| {
        !state["ime"]["area"].is_null()
    });
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
    // AppKit's current input context belongs to the key window, which the inner instance's
    // window takes when it opens, so on the native layer whether it is the bystander's follows
    // focus, not anything the bystander did.
    let mut changed = changed_fields(&before, &bystander.state());
    changed.retain(|field| field != "ime.current_input_context");
    changed.retain(|field| on_ci() || field == "buffers");
    assert!(
        changed.is_empty(),
        "the other instance's {changed:?} changed",
    );
}
