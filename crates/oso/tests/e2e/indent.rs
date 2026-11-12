//! Indentation in the running app (ADR 0115): in a visited Rust file, `RET` indents the new
//! line and opens one between brackets, `TAB` reindents, `C-j` breaks with no indentation, a
//! typed `}` goes under its opener, and a keyboard macro that types code replays with the same
//! indentation.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The rules and
//! each command's cases have their own tests.

use serde_json::json;

use super::harness::{App, Scratch};

/// The buffer's first `rows` rows.
fn rows(app: &mut App, rows: usize) -> serde_json::Value {
    app.call("get-text", Some(json!({ "start_row": 0, "rows": rows })))["rows"].clone()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn ret_tab_c_j_and_a_typed_closer_indent_a_rust_file() {
    let scratch = Scratch::new("indent");
    let file = scratch.write("main.rs", b"fn main() {}\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    // Between the braces: the closer on its own line, the cursor indented between.
    app.send_keys("C-e C-b RET");
    app.send_keys("l e t SPC x SPC = RET 1 ;");
    assert_eq!(
        rows(&mut app, 4),
        json!(["fn main() {", "    let x =", "        1;", "}"])
    );

    // TAB puts a line back; C-j breaks with no indentation.
    app.send_keys("C-a DEL DEL DEL DEL TAB");
    assert_eq!(rows(&mut app, 3)[2], "        1;");
    app.send_keys("C-e C-j");
    assert_eq!(rows(&mut app, 4)[3], "");

    // A typed closer first on its line goes under its opener's line.
    app.send_keys("SPC SPC SPC SPC SPC SPC }");
    assert_eq!(rows(&mut app, 4)[3], "}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_macro_that_types_code_replays_with_the_same_indentation() {
    let scratch = Scratch::new("indent-macro");
    let file = scratch.write("lib.rs", b"fn a() {}\nfn b() {}\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-x ( C-e C-b RET x ; C-n C-n C-x )");
    app.send_keys("C-x e");
    app.wait_for("the replay's end", |state| {
        state["macro"]["replaying"] == false
    });
    assert_eq!(
        rows(&mut app, 6),
        json!(["fn a() {", "    x;", "}", "fn b() {", "    x;", "}"])
    );
}
