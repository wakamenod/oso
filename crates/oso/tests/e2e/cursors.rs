//! The keys that add cursors through the socket's keys (ADR 0091): `C->` marks the next
//! occurrences, typing replaces them all, `C-g` hides the regions and then leaves one
//! cursor, and `C-x SPC` puts one on each line of a region.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Where each
//! key puts cursors is in the editor's and the dispatcher's tests.

use serde_json::Value;

use super::harness::{App, Scratch};

/// The selected window's cursors, as offsets.
fn cursors(state: &Value) -> Vec<u64> {
    state["windows"][0]["selections"]
        .as_array()
        .expect("the selections are listed")
        .iter()
        .map(|selection| selection["cursor"].as_u64().expect("a cursor is an offset"))
        .collect()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_greater_adds_cursors_that_type_together_and_c_g_takes_them_away() {
    let scratch = Scratch::new("cursors");
    let file = scratch.write("notes.txt", b"foo bar foo foo\none\ntwo\nthree\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-f C-> C->");
    let state = app.state();
    assert_eq!(cursors(&state), [3, 11, 15]);
    assert_eq!(state["windows"][0]["mark_active"], true);

    app.send_keys("C-g");
    let state = app.state();
    assert_eq!(state["windows"][0]["mark_active"], false);
    assert_eq!(
        cursors(&state),
        [3, 11, 15],
        "the first C-g hides the regions"
    );
    app.send_keys("x");
    assert_eq!(app.state()["text"]["rows"][0], "foox bar foox foox");
    app.send_keys("C-g");
    assert_eq!(cursors(&app.state()), [18]);

    app.send_keys("M-< C-n C-SPC C-n C-n C-f C-x SPC -");
    let state = app.state();
    assert_eq!(state["text"]["rows"][1], "o-ne");
    assert_eq!(state["text"]["rows"][2], "t-wo");
    assert_eq!(state["text"]["rows"][3], "t-hree");
    assert_eq!(cursors(&state).len(), 3);
}
