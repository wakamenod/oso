//! The recent files in the running app (ADR 0148): kept over a quit at once and a start again,
//! listed newest first by `C-x C-r` and after the buffers by `C-x b`, and a file gone since
//! dropped when chosen.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The list's order,
//! what it keeps and the file's format have their own tests.

use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::harness::{App, Scratch};
use super::typing;

/// The rows of the prompt that completes, as the dev state gives them.
fn rows(state: &Value) -> Vec<String> {
    state["prompts"][0]["completion"]["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Opens `C-x C-r` until it lists `count` files, as the read at start may still be on its way.
fn recent_rows(app: &mut App, count: usize) -> Vec<String> {
    let started = Instant::now();
    loop {
        app.send_keys("C-x C-r");
        let state = app.wait_for("the recent files", |state| {
            state["prompts"][0]["prompt"] == "Open recent file:"
        });
        let rows = rows(&state);
        if rows.len() == count || started.elapsed() > Duration::from_secs(10) {
            return rows;
        }
        app.send_keys("C-g");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn recent_files_are_kept_over_a_quit_and_open_from_c_x_c_r_and_c_x_b() {
    let scratch = Scratch::new("recent-files");
    let notes = scratch.write("notes.md", b"agenda\n");
    let plan = scratch.write("plan.md", b"budget\n");
    let gone = scratch.write("gone.md", b"old\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    for file in [&plan, &gone] {
        app.open_file(file);
        let name = file.file_name().unwrap().to_str().unwrap().to_owned();
        app.wait_for("the file loaded", |state| {
            state["buffer"]["name"] == name.as_str() && state["buffer"]["loading"].is_null()
        });
    }
    // At once: the exit waits for the recent files' write itself.
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
    std::fs::remove_file(&gone).unwrap();

    let mut again = App::start(None, scratch.path());
    again.wait_until_ready();
    let listed = recent_rows(&mut again, 3);
    let names: Vec<&str> = listed
        .iter()
        .map(|row| row.rsplit('/').next().unwrap_or(row))
        .collect();
    assert_eq!(names, ["gone.md", "plan.md", "notes.md"], "{listed:?}");

    // A file gone since says so, opens nothing and leaves the list.
    again.send_keys("RET");
    let state = again.wait_for("the word", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.ends_with("gone.md no longer exists"))
    });
    assert_eq!(state["buffer"]["name"], "*scratch*");
    assert_eq!(recent_rows(&mut again, 2).len(), 2);
    again.send_keys("RET");
    // Loaded, so judged: the state withholds a prompt opened beside a file not judged yet until
    // it closes.
    again.wait_for("plan.md open", |state| {
        state["buffer"]["name"] == "plan.md"
            && state["buffer"]["loading"].is_null()
            && state["prompts"] == serde_json::json!([])
    });

    // `C-x b` lists `notes.md` after the buffers, previews nothing of it, and opens it.
    again.send_keys("C-x b");
    again.send_keys(&typing("notes"));
    let state = again.wait_for("the file row", |state| {
        state["prompts"][0]["completion"]["preview_kind"] == "not-open"
    });
    assert!(rows(&state).iter().any(|row| row.ends_with("/notes.md")));
    again.send_keys("RET");
    again.wait_for("notes.md open", |state| {
        state["buffer"]["name"] == "notes.md" && state["buffer"]["loading"].is_null()
    });
}
