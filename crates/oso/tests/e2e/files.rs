//! Files in the running app (ADR 0087): `C-x C-f` listing a real directory on the lister's
//! threads, opening the file chosen and a new one, and `C-x C-w` writing under a new name.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Planning a path,
//! its segments and the save path have their own tests.

use serde_json::Value;

use super::harness::{App, Scratch};
use super::typing;

/// The state once the prompt that reads a path has its listing.
fn listed(app: &mut App) -> Value {
    app.wait_for("a listing", |state| {
        state["prompts"][0]["completion"]["pending"] == false
    })
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_c_f_lists_the_directory_and_opens_the_file_chosen() {
    let scratch = Scratch::new("files-find");
    let notes = scratch.write("notes.md", b"agenda\n");
    scratch.write("plan.md", b"budget\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-x C-f");
    let state = listed(&mut app);
    let completion = &state["prompts"][0]["completion"];
    let rows: Vec<&str> = completion["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(rows.contains(&"plan.md"), "{rows:?}");
    app.send_keys(&typing("plan"));
    app.send_keys("RET");
    let state = app.wait_until_ready();
    assert_eq!(state["prompts"], serde_json::json!([]));
    assert_eq!(state["buffer"]["name"], "plan.md");

    // A name that matches nothing opens as a new file, which nothing writes yet.
    app.send_keys("C-x C-f");
    listed(&mut app);
    app.send_keys(&typing("fresh.md"));
    app.send_keys("RET");
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["name"], "fresh.md");
    assert!(!scratch.path().join("fresh.md").exists());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_c_w_writes_under_a_new_name_and_the_buffer_visits_it() {
    let scratch = Scratch::new("files-write");
    let notes = scratch.write("notes.md", b"agenda\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    app.send_keys("x C-x C-w");
    listed(&mut app);
    app.send_keys(&typing("copy.md"));
    app.send_keys("RET");
    let state = app.wait_for("the write", |state| {
        state["buffer"]["name"] == "copy.md" && state["buffer"]["modified"] == false
    });
    assert_eq!(state["buffer"]["name"], "copy.md");
    let copy = std::fs::read_to_string(scratch.path().join("copy.md")).unwrap();
    assert_eq!(copy, "xagenda\n");
    assert_eq!(
        std::fs::read_to_string(&notes).unwrap(),
        "agenda\n",
        "untouched"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_link_to_a_file_open_already_shows_that_buffer() {
    let scratch = Scratch::new("files-link");
    let notes = scratch.write("notes.md", b"agenda\n");
    scratch.symlink("link.md", &notes);
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    app.send_keys("C-x C-f");
    listed(&mut app);
    app.send_keys(&typing("link.md"));
    app.send_keys("M-RET");
    let state = app.wait_for("the open buffer", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("already open"))
    });
    assert_eq!(state["buffer"]["name"], "notes.md");
    let names: Vec<&Value> = state["buffers"]
        .as_array()
        .expect("buffers")
        .iter()
        .map(|buffer| &buffer["name"])
        .collect();
    assert!(!names.iter().any(|name| *name == "link.md"), "{names:?}");
}
