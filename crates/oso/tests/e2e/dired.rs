//! The directory editor in the running app (ADR 0134): `C-x C-j` and `C-x d` listing a real
//! directory on the lister's threads, `RET` and `^` moving between directories in one buffer,
//! opening a file, the watcher's refresh, `s` and `(`, a listing that may hold secrets, and a
//! listing over a restart.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The rows' text,
//! their order and the platform's listing have their own tests.

use serde_json::{Value, json};

use super::handover::hand_over;
use super::harness::{App, Scratch};
use super::typing;

/// The shown buffer's rows, whole.
fn rows(app: &mut App) -> Vec<String> {
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 1000 })));
    text["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The row whose name is `name`, by its end.
fn row_of(rows: &[String], name: &str) -> usize {
    rows.iter()
        .position(|row| row.ends_with(&format!("  {name}")))
        .unwrap_or_else(|| panic!("no row for {name} in {rows:?}"))
}

/// Waits for the listing named `name` to show with a row ending in `last`.
fn listed(app: &mut App, name: &str, last: &str) -> Value {
    let name = name.to_owned();
    let state = app.wait_for("a listing", |state| {
        state["buffer"]["name"] == name.as_str()
    });
    let started = std::time::Instant::now();
    while !rows(app).iter().any(|row| row.ends_with(last)) {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "no {last} in {name}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    state
}

/// Moves point by rows from where it is to `row`, with `n` and `p`.
fn go_to_row(app: &mut App, row: usize) {
    let at = app.state()["cursor"]["row"].as_u64().unwrap() as usize;
    let keys: Vec<&str> = if row > at {
        vec!["n"; row - at]
    } else {
        vec!["p"; at - row]
    };
    if !keys.is_empty() {
        app.send_keys(&keys.join(" "));
    }
    app.wait_for("point on the row", |state| state["cursor"]["row"] == row);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_c_j_lists_the_directory_and_moves_between_directories_in_one_buffer() {
    let scratch = Scratch::new("dired-move");
    let notes = scratch.write("notes.md", b"agenda\n");
    scratch.write("plan.md~", b"backup\n");
    std::fs::create_dir(scratch.path().join("sub")).unwrap();
    scratch.write("sub/inner.txt", b"inside\n");
    let name = format!("{}/", scratch.path().file_name().unwrap().to_string_lossy());
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-x C-j");
    let state = listed(&mut app, &name, "notes.md");
    assert!(
        state["modeline"]
            .as_str()
            .unwrap()
            .contains("Dired by name"),
        "{state}"
    );
    let shown = rows(&mut app);
    assert!(
        !shown.iter().any(|row| row.contains("plan.md~")),
        "{shown:?}"
    );
    assert!(
        row_of(&shown, "sub") < row_of(&shown, "notes.md"),
        "directories first"
    );
    assert_eq!(
        app.state()["cursor"]["row"],
        row_of(&shown, "notes.md"),
        "point on the file"
    );

    // `RET` on a directory lists it in the same buffer, and `^` comes back to it.
    let buffers = app.state()["buffers"].as_array().unwrap().len();
    go_to_row(&mut app, row_of(&shown, "sub"));
    app.send_keys("RET");
    listed(&mut app, "sub/", "inner.txt");
    assert_eq!(app.state()["buffers"].as_array().unwrap().len(), buffers);
    app.send_keys("^");
    listed(&mut app, &name, "notes.md");
    assert_eq!(app.state()["cursor"]["row"], row_of(&shown, "sub"));

    // `RET` on a file opens it.
    go_to_row(&mut app, row_of(&shown, "notes.md"));
    app.send_keys("RET");
    let state = app.wait_for("the file", |state| state["buffer"]["name"] == "notes.md");
    assert_eq!(state["buffer"]["path"], notes.to_string_lossy().as_ref());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_made_outside_shows_on_its_own_and_s_and_paren_change_the_rows() {
    let scratch = Scratch::new("dired-watch");
    let notes = scratch.write("notes.md", b"agenda\n");
    let name = format!("{}/", scratch.path().file_name().unwrap().to_string_lossy());
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-x C-j");
    listed(&mut app, &name, "notes.md");
    let point = app.state()["cursor"]["row"].clone();

    scratch.write("fresh.md", b"new\n");
    listed(&mut app, &name, "fresh.md");
    let shown = rows(&mut app);
    assert_eq!(
        app.state()["cursor"]["row"],
        row_of(&shown, "notes.md"),
        "point kept"
    );
    assert_ne!(point, Value::Null);

    app.send_keys("s");
    app.wait_for("by date", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("Dired by date"))
    });
    app.send_keys("(");
    let started = std::time::Instant::now();
    while rows(&mut app).iter().any(|row| row.contains("rw")) {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "details hidden"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(rows(&mut app).iter().any(|row| row == "  notes.md"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_listing_inside_ssh_gives_the_socket_no_text() {
    let scratch = Scratch::new("dired-secret");
    let notes = scratch.write("notes.md", b"agenda\n");
    std::fs::create_dir(scratch.path().join(".ssh")).unwrap();
    scratch.write(".ssh/id_listed_key", b"not read\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    let directory = scratch.path().join(".ssh");
    app.send_keys("C-x d C-a C-k");
    app.send_keys(&typing(&directory.to_string_lossy()));
    app.send_keys("M-RET");
    let state = app.wait_for("the listing", |state| state["buffer"]["name"] == ".ssh/");
    assert_eq!(state["buffer"]["secret"], true, "{state}");
    assert!(!state.to_string().contains("id_listed_key"), "{state}");
    let text = app.try_call("get-text", Some(json!({ "start_row": 0, "rows": 100 })));
    assert!(!format!("{text:?}").contains("id_listed_key"), "{text:?}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_listing_comes_back_after_a_restart_listed_again() {
    let scratch = Scratch::new("dired-restart");
    let notes = scratch.write("notes.md", b"agenda\n");
    scratch.write("plan.md", b"budget\n");
    let name = format!("{}/", scratch.path().file_name().unwrap().to_string_lossy());
    let mut old = App::start(Some(&notes), scratch.path());
    old.wait_until_ready();
    old.send_keys("C-x C-j");
    listed(&mut old, &name, "plan.md");
    let shown = rows(&mut old);
    go_to_row(&mut old, row_of(&shown, "plan.md"));
    old.send_keys("s");
    old.wait_for("by date", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("by date"))
    });
    let row = old.state()["cursor"]["row"].clone();

    let mut new = App::start(None, scratch.path());
    new.wait_until_ready();
    hand_over(&mut old, &mut new);
    let state = listed(&mut new, &name, "plan.md");
    assert!(
        state["modeline"].as_str().unwrap().contains("by date"),
        "{state}"
    );
    new.wait_for("point where it was", |state| state["cursor"]["row"] == row);
}
