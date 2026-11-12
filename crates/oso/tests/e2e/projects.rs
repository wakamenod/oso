//! A project's search in the running app (ADR 0114): `M-s r` over a real directory on the
//! searching threads, the preview of each row, `RET` visiting the line found, and `C-u`'s
//! directory prompt.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The query, the
//! walk and the reading of files have their own tests.

use serde_json::Value;

use super::harness::{App, Scratch};
use super::{cursor, typing};

/// The rows of the innermost prompt once its search has ended.
fn searched(app: &mut App) -> Vec<String> {
    let state = app.wait_for("a search", |state| {
        state["prompts"][0]["completion"]["pending"] == false
            && state["prompts"][0]["completion"]["total"]
                .as_u64()
                .is_some_and(|total| total > 0)
    });
    rows(&state)
}

fn rows(state: &Value) -> Vec<String> {
    state["prompts"][0]["completion"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// A project with a `.git`, two files that hold `needle` and one that may hold secrets.
fn project(scratch: &Scratch) -> std::path::PathBuf {
    let main = scratch.write(
        "proj-search/src/main.rs",
        b"fn main() {\n    let needle = 1;\n}\n",
    );
    scratch.write("proj-search/docs/notes.md", b"# Notes\n\nA needle here.\n");
    scratch.write("proj-search/.env", b"TOKEN=needle\n");
    std::fs::create_dir(scratch.path().join("proj-search/.git")).unwrap();
    main
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_s_r_searches_the_project_and_ret_visits_the_line() {
    let scratch = Scratch::new("projects-search");
    let main = project(&scratch);
    let mut app = App::start(Some(&main), scratch.path());
    app.wait_until_ready();
    app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  proj-search  "))
    });

    app.send_keys("M-s r");
    app.send_keys(&typing("needle"));
    let mut found = searched(&mut app);
    found.sort();
    assert_eq!(
        found,
        [
            ".env:1",
            "docs/notes.md:3: A needle here.",
            "src/main.rs:2:     let needle = 1;"
        ]
    );
    // The row of `notes.md`, wherever it came.
    let state = app.state();
    let place = rows(&state)
        .iter()
        .position(|row| row.starts_with("docs/"))
        .expect("the row");
    for _ in 0..place {
        app.send_keys("C-n");
    }
    app.send_keys("RET");
    app.wait_for("the visit", |state| {
        state["buffer"]["name"] == "notes.md" && state["buffer"]["loading"].is_null()
    });
    let state = app.wait_for("the line", |state| cursor(state) == (2, 2));
    assert!(state["prompts"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(state["cursor"]["mark"]["offset"], 0, "the place before");
    let log = app.stop_and_read_log();
    assert!(!log.contains("needle"), "nothing of the search is logged");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_u_asks_for_a_directory_refuses_a_file_and_searches_the_one_typed() {
    let scratch = Scratch::new("projects-directory");
    let main = project(&scratch);
    let mut app = App::start(Some(&main), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-u M-s r");
    let state = app.wait_for("the prompt", |state| {
        state["prompts"][0]["prompt"] == "Search in directory:"
            && state["prompts"][0]["completion"]["pending"] == false
    });
    assert!(
        state["prompts"][0]["text"]
            .as_str()
            .is_some_and(|text| text.ends_with("src/"))
    );
    // It lists no file: `src/` holds only one.
    assert_eq!(rows(&state), Vec::<String>::new());
    // A file typed whole is no directory: the prompt stays open and says so.
    app.send_keys(&typing("main.rs"));
    app.send_keys("RET");
    let state = app.wait_for("the refusal", |state| {
        state["prompts"][0]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("Not a directory: "))
    });
    assert_eq!(state["prompts"][0]["prompt"], "Search in directory:");
    // The project's directories are listed, and none of its files.
    app.send_keys("C-a C-k");
    let root = scratch.path().join("proj-search/");
    app.send_keys(&typing(root.to_str().expect("a UTF-8 scratch path")));
    let state = app.wait_for("the project's listing", |state| {
        state["prompts"][0]["completion"]["pending"] == false
            && state["prompts"][0]["completion"]["total"] == 2
    });
    // Shorter first, as completion sorts (ADR 0076).
    assert_eq!(rows(&state), ["src/", "docs/"]);
    // A directory typed whole, `/` at its end, is searched.
    app.send_keys("C-a C-k");
    let docs = scratch.path().join("proj-search/docs/");
    app.send_keys(&typing(docs.to_str().expect("a UTF-8 scratch path")));
    app.send_keys("RET");
    app.wait_for("the search's prompt", |state| {
        state["prompts"][0]["prompt"] == "Search docs:"
    });
    app.send_keys(&typing("needle"));
    assert_eq!(searched(&mut app), ["notes.md:3: A needle here."]);
}

/// The kind of thing the preview shows once it has settled on `kind`.
fn previewed(app: &mut App, kind: &str) -> Value {
    app.wait_for(kind, |state| {
        state["prompts"][0]["completion"]["preview_kind"] == kind
    })
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_s_r_previews_each_row_but_never_a_file_that_may_hold_secrets() {
    let scratch = Scratch::new("projects-preview");
    let main = project(&scratch);
    let mut app = App::start(Some(&main), scratch.path());
    app.wait_until_ready();
    app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  proj-search  "))
    });
    app.send_keys("M-s r");
    app.send_keys(&typing("needle"));
    let found = searched(&mut app);
    // Each row, from the first down: the open buffer of `main.rs` from memory, `notes.md` read
    // from disk, and `.env` never.
    for (place, row) in found.iter().enumerate() {
        let kind = if row.starts_with("src/") {
            "buffer"
        } else if row.starts_with("docs/") {
            "excerpt"
        } else {
            "withheld"
        };
        let state = previewed(&mut app, kind);
        assert_eq!(state["prompts"][0]["completion"]["selected"], place);
        if kind != "buffer" {
            assert!(state["prompts"][0]["completion"]["preview"].is_null());
        }
        app.send_keys("C-n");
    }
    app.send_keys("C-g");
    let log = app.stop_and_read_log();
    assert!(
        !log.contains("needle") && !log.contains("TOKEN"),
        "nothing read is logged"
    );
}
