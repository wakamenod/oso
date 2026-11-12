//! Highlighting through a running instance (ADR 0111): a visited Rust file is highlighted and
//! drawn in colour, which `global-font-lock-mode` turns off and on. That a buffer that may hold
//! secrets is never parsed is pinned in the glue's unit tests, where it can fail.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Which glyph takes
//! which face, and the colours, are in oso-render's tests.

use serde_json::Value;

use super::harness::{App, Scratch};

const CODE: &str =
    "// A comment.\nfn main() {\n    let name = \"oso\";\n    println!(\"{name}\");\n}\n";

fn highlighted(state: &Value) -> bool {
    state["buffer"]["highlighted"] == true
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_rust_file_is_drawn_in_colour_while_font_lock_is_on() {
    let scratch = Scratch::new("highlight");
    let file = scratch.write("main.rs", CODE.as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.wait_for("highlights", highlighted);
    let coloured = app.screenshot();

    app.run_command("global-font-lock-mode", None).unwrap();
    let state = app.state();
    assert!(!highlighted(&state), "turned off, the highlights go");
    let plain = app.screenshot();
    // The text's rows, above the modeline and the echo area, which says what changed.
    let text = 0..plain.height / 2;
    assert!(
        coloured.differs_in(&plain, text.clone()) > 0,
        "the colour is drawn, and goes with font-lock"
    );

    app.run_command("global-font-lock-mode", None).unwrap();
    app.wait_for("highlights again", highlighted);
    assert_eq!(app.screenshot().differs_in(&coloured, text), 0);
}
