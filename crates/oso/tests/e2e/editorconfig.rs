//! EditorConfig in the running app (ADR 0115): a file visited under an `.editorconfig` is
//! shown with its tab width and indented with tabs, `describe-indentation` names the file in
//! the project, and `C-x C-w` into a directory with another `.editorconfig` takes its
//! indentation when the read comes back.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The parser, the
//! walk and the precedence have their own tests.

use serde_json::{Value, json};

use super::harness::{App, Scratch};
use super::typing;

/// The buffer's first `rows` rows.
fn rows(app: &mut App, rows: usize) -> Value {
    app.call("get-text", Some(json!({ "start_row": 0, "rows": rows })))["rows"].clone()
}

/// The echo area once it says something that starts with `start`.
fn echoed(app: &mut App, start: &str) -> String {
    let state = app.wait_for("the echo", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with(start))
    });
    state["echo"].as_str().unwrap_or_default().to_owned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_under_an_editorconfig_takes_its_tab_width_and_indents_with_tabs() {
    let scratch = Scratch::new("editorconfig");
    scratch.write(
        "proj/.editorconfig",
        b"root = true\n\n[*.rs]\nindent_style = tab\ntab_width = 4\n",
    );
    std::fs::create_dir(scratch.path().join("proj/.git")).unwrap();
    let file = scratch.write("proj/src/main.rs", b"fn main() {\n\tlet x = 1;\n}\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    // A tab four columns wide.
    app.send_keys("C-n C-a C-f");
    assert_eq!(app.state()["cursor"]["column"], 4);

    // `RET` indents the new line with a tab, and `TAB` puts spaces back to one.
    app.send_keys("C-e RET y ; RET SPC SPC z ; TAB");
    assert_eq!(
        rows(&mut app, 5),
        json!(["fn main() {", "\tlet x = 1;", "\ty;", "\tz;", "}"])
    );

    // Named by its path in the project, once the project is found.
    app.wait_for("the project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  proj  "))
    });
    app.run_command("describe-indentation", None).unwrap();
    assert_eq!(
        echoed(&mut app, "Rust mode"),
        "Rust mode: indent size 4 (EditorConfig .editorconfig), tabs (EditorConfig \
         .editorconfig), tab width 4 (EditorConfig .editorconfig)"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_c_w_takes_the_indentation_of_where_the_file_now_is() {
    let scratch = Scratch::new("editorconfig-write");
    scratch.write(
        "tabs/.editorconfig",
        b"root = true\n[*]\nindent_style = tab\n",
    );
    let notes = scratch.write("plain/notes.rs", b"fn f() {\nx;\n}\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-x C-w");
    app.wait_for("a listing", |state| {
        state["prompts"][0]["completion"]["pending"] == false
    });
    // The prompt starts in the file's directory.
    app.send_keys(&typing("../tabs/notes.rs"));
    app.send_keys("RET");
    app.wait_for("the write", |state| {
        state["buffer"]["name"] == "notes.rs" && state["buffer"]["modified"] == false
    });
    // The read comes back on a thread of its own.
    let mut said = String::new();
    for _ in 0..100 {
        app.run_command("describe-indentation", None).unwrap();
        said = echoed(&mut app, "Rust mode");
        if said.contains("tabs") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        said.contains("tabs (EditorConfig outside the project)"),
        "{said}"
    );
    app.send_keys("C-n TAB");
    assert_eq!(rows(&mut app, 3), json!(["fn f() {", "\tx;", "}"]));
}
