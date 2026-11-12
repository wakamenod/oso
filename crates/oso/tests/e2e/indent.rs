//! Indentation in the running app (ADR 0115): in a visited Rust file, `RET` indents the new
//! line and opens one between brackets, `TAB` reindents, `C-j` breaks with no indentation, a
//! typed `}` goes under its opener, and a keyboard macro that types code replays with the same
//! indentation; `C-M-\` reindents a region, `C-x TAB`'s arrows shift one as one undo step, and
//! `M-m` goes past a line's indentation.
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

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_region_keys_and_m_m_indent_a_rust_file() {
    let scratch = Scratch::new("indent-region");
    let file = scratch.write("lib.rs", b"fn f() {\nlet x = 1;\n  if x {\ny();\n}\n}\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let reindented = json!([
        "fn f() {",
        "    let x = 1;",
        "    if x {",
        "        y();",
        "    }",
        "}"
    ]);

    app.send_keys("C-x h C-M-\\");
    assert_eq!(rows(&mut app, 6), reindented);

    // The arrows of C-x TAB shift the region's lines while it stays shown.
    app.send_keys("M-< C-n C-SPC C-n C-n C-x TAB <right> <right> S-<right>");
    assert_eq!(
        rows(&mut app, 4),
        json!([
            "fn f() {",
            "        let x = 1;",
            "        if x {",
            "        y();"
        ])
    );
    assert!(
        !app.state()["cursor"]["mark"].is_null(),
        "the region has its mark"
    );
    // Another key closes the layer and runs as usual; the shifts undo as one step.
    app.send_keys("C-g C-z");
    assert_eq!(rows(&mut app, 6), reindented);

    // M-m goes past the indentation.
    app.send_keys("M-< C-n C-e M-m");
    let cursor = app.state()["cursor"].clone();
    assert_eq!(
        (cursor["row"].clone(), cursor["column"].clone()),
        (json!(1), json!(4))
    );
}
