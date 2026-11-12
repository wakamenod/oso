//! Projects in the running app (ADR 0114): `C-x p f` over a real directory on the walking
//! thread, `M-s r` over it on the searching threads, the preview of each row, `RET` visiting
//! the file or the line found, `C-u`'s directory prompt, and `trust-project` and
//! `untrust-project` with the record they write beside the configuration file.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The query, the
//! walk, the reading of files and the record's lines have their own tests.

use std::path::Path;

use serde_json::Value;

use super::config::set_mode;
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

/// The rows of the innermost prompt once its walk has ended with `total` files.
fn walked(app: &mut App, total: u64) -> Vec<String> {
    let state = app.wait_for("the walk", |state| {
        state["prompts"][0]["completion"]["pending"] == false
            && state["prompts"][0]["completion"]["total"] == total
    });
    rows(&state)
}

/// Waits until the modeline names the project `name`, then says whether it says `restricted`.
fn restricted(app: &mut App, name: &str) -> bool {
    let state = app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains(&format!("  {name}  ")))
    });
    let modeline = state["modeline"].as_str().unwrap_or_default();
    modeline.contains(&format!("  {name}  restricted  "))
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

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_p_f_lists_the_projects_files_from_its_root_and_ret_visits_one() {
    let scratch = Scratch::new("projects-find");
    let main = project(&scratch);
    let mut app = App::start(Some(&main), scratch.path());
    app.wait_until_ready();
    restricted(&mut app, "proj-search");

    app.send_keys("C-x p f");
    let state = app.state();
    assert_eq!(state["prompts"][0]["prompt"], "Find file in proj-search:");
    // Every file from the root, `.env` too, and nothing of `.git`; shorter first (ADR 0076).
    assert_eq!(
        walked(&mut app, 3),
        [".env", "src/main.rs", "docs/notes.md"]
    );
    app.send_keys(&typing("notes"));
    let state = app.wait_for("the match", |state| {
        state["prompts"][0]["completion"]["count"] == 1
    });
    assert_eq!(rows(&state), ["docs/notes.md"]);
    app.send_keys("RET");
    let state = app.wait_for("the visit", |state| {
        state["buffer"]["name"] == "notes.md" && state["buffer"]["loading"].is_null()
    });
    assert!(state["prompts"].as_array().is_some_and(Vec::is_empty));
    let notes = scratch.path().join("proj-search/docs/notes.md");
    assert_eq!(
        state["buffer"]["path"],
        notes.to_str().expect("a UTF-8 scratch path")
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_u_c_x_p_f_lists_only_directories_then_the_one_given_as_a_project() {
    let scratch = Scratch::new("projects-find-directory");
    let main = project(&scratch);
    let mut app = App::start(Some(&main), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-u C-x p f");
    let state = app.wait_for("the prompt", |state| {
        state["prompts"][0]["prompt"] == "Find file in directory:"
            && state["prompts"][0]["completion"]["pending"] == false
    });
    assert!(
        state["prompts"][0]["text"]
            .as_str()
            .is_some_and(|text| text.ends_with("proj-search/src/"))
    );
    // Up to the project's root, whose directories are listed and none of its files.
    app.send_keys("DEL DEL DEL DEL");
    let state = app.wait_for("the root's listing", |state| {
        state["prompts"][0]["completion"]["pending"] == false
            && state["prompts"][0]["completion"]["total"] == 2
    });
    assert_eq!(rows(&state), ["src/", "docs/"]);
    // A directory typed whole is listed as a project would be, its files from it.
    app.send_keys(&typing("docs/"));
    app.send_keys("RET");
    app.wait_for("the listing's prompt", |state| {
        state["prompts"][0]["prompt"] == "Find file in docs:"
    });
    assert_eq!(walked(&mut app, 1), ["notes.md"]);
    app.send_keys("RET");
    app.wait_for("the visit", |state| {
        state["buffer"]["name"] == "notes.md" && state["buffer"]["loading"].is_null()
    });
}

/// The record's line for `root`, by its path and its directory's device and inode.
fn record_line(root: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(root).expect("the scratch project");
    let path = root.to_str().expect("a UTF-8 scratch path");
    format!("{path}\tdev={} ino={}", meta.dev(), meta.ino())
}

/// Waits until the record, if there is one, holds `line` or not, as `held` says.
fn wait_for_record(app: &mut App, record: &Path, line: &str, held: bool) {
    let what = if held {
        "the line written"
    } else {
        "the line removed"
    };
    app.wait_for(what, |_| {
        std::fs::read_to_string(record).is_ok_and(|text| text.lines().any(|l| l == line) == held)
    });
}

/// Runs `command` from `M-x`, as a person does.
fn m_x(app: &mut App, command: &str) {
    app.send_keys(&format!("M-x {} RET", typing(command)));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn trust_project_asks_records_the_root_beside_the_config_and_untrust_removes_it() {
    let scratch = Scratch::new("projects-trust");
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("config.toml", b"");
    set_mode(&config, 0o644);
    let record = scratch.path().join("trusted-projects");
    let main = scratch.write("proj-trust/src/main.rs", b"fn main() {}\n");
    let root = scratch.path().join("proj-trust");
    std::fs::create_dir(root.join(".git")).unwrap();
    let line = record_line(&root);

    let mut app = App::start_with_config(Some(&main), scratch.path(), &config);
    app.wait_until_ready();
    assert!(
        restricted(&mut app, "proj-trust"),
        "a new project is restricted"
    );
    assert!(!record.exists(), "nothing is written before a yes");

    m_x(&mut app, "trust-project");
    let state = app.state();
    let question = &state["prompts"][0];
    assert_eq!(question["kind"], "question");
    let label = question["prompt"].as_str().unwrap_or_default();
    assert!(
        label.starts_with("Trust ")
            && label.contains(
                "proj-trust? Its code may run through language servers, formatters and git hooks"
            ),
        "{label}"
    );
    // `y` records the root the question named, by its path and its directory's identity.
    app.send_keys("y");
    let state = app.wait_for("the trust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Trusted ") && echo.ends_with("proj-trust"))
    });
    assert!(state["prompts"].as_array().is_some_and(Vec::is_empty));
    wait_for_record(&mut app, &record, &line, true);
    assert!(!restricted(&mut app, "proj-trust"), "trusted");
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(text.starts_with("# Projects oso trusts"), "the header");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&record).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the record is private");
    app.stop_and_read_log();

    // A new instance reads the record back, and `untrust-project` takes the line out of it.
    let mut app = App::start_with_config(Some(&main), scratch.path(), &config);
    app.wait_until_ready();
    app.wait_for("the record read", |state| {
        state["modeline"].as_str().is_some_and(|modeline| {
            modeline.contains("  proj-trust  ") && !modeline.contains("restricted")
        })
    });
    m_x(&mut app, "untrust-project");
    let state = app.wait_for("the untrust", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.ends_with("proj-trust is restricted again"))
    });
    assert!(
        state["prompts"].as_array().is_some_and(Vec::is_empty),
        "no question"
    );
    wait_for_record(&mut app, &record, &line, false);
    assert!(restricted(&mut app, "proj-trust"), "restricted again");
}
