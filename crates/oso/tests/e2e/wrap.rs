//! Soft wrap through a running instance (ADR 0100, ADR 0103): `toggle-truncate-lines` wraps a
//! long line, the state says so, a screen scrolls by view rows, and `C-n` still goes to the next
//! logical line.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Where rows break
//! and how scrolling counts them are in oso-core's and the dispatcher's tests.

use serde_json::Value;

use super::harness::{App, Scratch};
use super::{cursor, scroll_top};

fn wrapped(state: &Value) -> bool {
    state["buffer"]["wrapped"] == true
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_wrapped_line_scrolls_by_view_rows_and_c_n_goes_to_the_next_line() {
    let scratch = Scratch::new("wrap");
    let text = format!("{}\nnext line\n", "word ".repeat(2000));
    let file = scratch.write("prose.txt", text.as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    let state = app.wait_until_ready();
    assert!(!wrapped(&state), "off by default");
    let rows = state["window"]["rows"].as_u64().expect("the window's rows");

    app.run_command("toggle-truncate-lines", None)
        .expect("the command runs");
    let state = app.state();
    assert!(wrapped(&state));
    assert_eq!(state["echo"], "Truncate long lines disabled");
    assert_eq!(scroll_top(&state), 0);

    app.send_keys("C-v");
    let state = app.state();
    assert_eq!(
        scroll_top(&state),
        rows - 2,
        "a screen of view rows, less two of context"
    );
    let start = state["scroll"]["start"].as_u64().expect("the top's offset");
    assert!(start > 0, "the top is inside the long line");
    let (row, column) = cursor(&state);
    assert_eq!(row, 0, "the cursor stays on the long line");
    assert_eq!(column, start, "at the start of the top row");

    app.send_keys("C-n");
    assert_eq!(cursor(&app.state()).0, 1, "the next logical line");

    app.run_command("toggle-truncate-lines", Some(1))
        .expect("the command runs");
    let state = app.state();
    assert!(!wrapped(&state), "a positive argument truncates");
    assert_eq!(state["echo"], "Truncate long lines enabled");
}
