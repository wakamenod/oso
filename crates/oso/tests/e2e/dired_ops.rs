//! The directory editor's file operations in the running app (ADR 0134, step 1b-2): `D` after
//! its question, `C` to the next window's listing with the question before writing over, `R` of
//! a file whose buffer follows it, the refusal over unsaved edits, and `+`.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The operations
//! themselves have their own tests.

use serde_json::{Value, json};

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

fn row_of(rows: &[String], name: &str) -> Option<usize> {
    rows.iter()
        .position(|row| row.ends_with(&format!("  {name}")))
}

/// Waits until the shown listing's rows `done` holds of them.
fn rows_until(app: &mut App, what: &str, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    let started = std::time::Instant::now();
    loop {
        let shown = rows(app);
        if done(&shown) {
            return shown;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "no {what}: {shown:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn go_to(app: &mut App, name: &str) {
    let shown = rows(app);
    let row = row_of(&shown, name).unwrap_or_else(|| panic!("no {name}: {shown:?}"));
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

/// Waits for a question and returns what it asks.
fn question(app: &mut App) -> String {
    let state = app.wait_for("a question", |state| {
        state["prompts"][0]["kind"] == "question"
    });
    state["prompts"][0]["prompt"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn d_deletes_the_marked_rows_after_its_question() {
    let scratch = Scratch::new("dired-delete");
    let notes = scratch.write("notes.md", b"agenda\n");
    scratch.write("a.txt", b"a\n");
    scratch.write("b.txt", b"b\n");
    std::fs::create_dir(scratch.path().join("old")).unwrap();
    scratch.write("old/inner.txt", b"x\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-x C-j");
    rows_until(&mut app, "the listing", |shown| {
        row_of(shown, "b.txt").is_some()
    });
    go_to(&mut app, "a.txt");
    app.send_keys("m");
    go_to(&mut app, "old");
    app.send_keys("m");
    app.send_keys("D");
    let asked = question(&mut app);
    assert!(asked.starts_with("Delete 2 files (old/, a.txt)"), "{asked}");
    assert!(asked.contains("all the directories hold"), "{asked}");
    app.send_keys("y");
    rows_until(&mut app, "the rows gone", |shown| {
        row_of(shown, "a.txt").is_none() && row_of(shown, "old").is_none()
    });
    assert!(!scratch.path().join("a.txt").exists());
    assert!(!scratch.path().join("old").exists());
    assert!(scratch.path().join("b.txt").exists());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_copies_to_the_next_windows_listing_and_asks_before_writing_over() {
    let scratch = Scratch::new("dired-copy");
    let notes = scratch.write("from/notes.md", b"agenda\n");
    scratch.write("to/notes.md", b"old\n");
    let to = scratch.path().join("to");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    // The listing of `to/` in the right window, of `from/` in the left.
    app.send_keys("C-x 3 C-x o C-x d C-a C-k");
    app.send_keys(&typing(&to.to_string_lossy()));
    app.send_keys("M-RET");
    rows_until(&mut app, "the target's listing", |shown| {
        row_of(shown, "notes.md").is_some()
    });
    app.send_keys("C-x o C-x C-j");
    rows_until(&mut app, "the source's listing", |shown| {
        row_of(shown, "notes.md").is_some()
    });
    go_to(&mut app, "notes.md");
    app.send_keys("C");
    let state = app.wait_for("the target prompt", |state| {
        state["prompts"][0]["text"]
            .as_str()
            .is_some_and(|answer| answer.contains("to"))
    });
    let answer = state["prompts"][0]["text"].as_str().unwrap().to_owned();
    assert!(answer.trim_end_matches('/').ends_with("to"), "{answer}");
    app.send_keys("M-RET");
    let asked = question(&mut app);
    assert!(
        asked.starts_with("1 file (notes.md) there already"),
        "{asked}"
    );
    app.send_keys("y");
    let started = std::time::Instant::now();
    while std::fs::read_to_string(to.join("notes.md")).unwrap() != "agenda\n" {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "copied"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn r_moves_a_file_whose_buffer_follows_and_refuses_one_with_unsaved_edits() {
    let scratch = Scratch::new("dired-move");
    let notes = scratch.write("notes.md", b"agenda\n");
    let plan = scratch.write("plan.md", b"budget\n");
    std::fs::create_dir(scratch.path().join("sub")).unwrap();
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    // `plan.md` open with unsaved edits, then back to the listing.
    app.open_file(&plan);
    app.wait_for("plan.md", |state| state["buffer"]["name"] == "plan.md");
    app.send_keys("x C-x C-j");
    rows_until(&mut app, "the listing", |shown| {
        row_of(shown, "sub").is_some()
    });
    go_to(&mut app, "plan.md");
    app.send_keys("R C-a C-k");
    app.send_keys(&typing(&scratch.path().join("sub").to_string_lossy()));
    app.send_keys("M-RET");
    app.wait_for("the refusal", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("unsaved edits"))
    });
    assert!(plan.exists());
    // `notes.md` has no edits: it moves, and its buffer follows.
    go_to(&mut app, "notes.md");
    app.send_keys("R C-a C-k");
    app.send_keys(&typing(&scratch.path().join("sub").to_string_lossy()));
    app.send_keys("M-RET");
    let moved = scratch.path().join("sub").join("notes.md");
    rows_until(&mut app, "the row gone", |shown| {
        row_of(shown, "notes.md").is_none()
    });
    assert!(moved.exists());
    let state = app.state();
    let buffers = state["buffers"].as_array().unwrap();
    // The buffer visits its file's real place now.
    let real = std::fs::canonicalize(&moved).unwrap();
    let followed = buffers
        .iter()
        .any(|buffer| buffer["path"].as_str() == Some(real.to_string_lossy().as_ref()));
    assert!(followed, "{buffers:?}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn plus_makes_a_directory_with_its_parents() {
    let scratch = Scratch::new("dired-mkdir");
    let notes = scratch.write("notes.md", b"agenda\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-x C-j");
    rows_until(&mut app, "the listing", |shown| {
        row_of(shown, "notes.md").is_some()
    });
    app.send_keys("+ C-a C-k");
    let made = scratch.path().join("x").join("y");
    app.send_keys(&typing(&made.to_string_lossy()));
    app.send_keys("M-RET");
    rows_until(&mut app, "the new directory", |shown| {
        row_of(shown, "x").is_some()
    });
    assert!(made.is_dir());
}
