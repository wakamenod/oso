//! The mark and the region through the socket's keys (ADR 0071): a shifted key
//! that only its unshifted form binds selects, the region is drawn in the window
//! and hides on `C-g`, and a buffer that may hold secrets shows nothing of it.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Which
//! cells a region fills is in the text-grid snapshots.

use serde_json::Value;

use super::harness::{App, FAKE_SECRET, Scratch};

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_region_from_shifted_keys_is_drawn_and_a_secret_buffer_shows_none_of_it() {
    let scratch = Scratch::new("mark-region");
    let file = scratch.write("notes.txt", b"abc\ndef\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.visual_aids_off();
    let state = app.wait_until_ready();
    let notes = state["buffer"]["id"].clone();
    let before = app.screenshot().foreground().expect("the text is drawn");

    // `S-<down>` and `C-F` are bound to nothing, so they run `next-line` and
    // `forward-char` as shifted.
    app.send_keys("S-<down> C-F");
    let state = app.state();
    assert_eq!(state["windows"][0]["mark_active"], true);
    assert_eq!(state["cursor"]["mark"]["offset"], 0);
    assert_eq!(state["cursor"]["offset"], 5);
    // The first row is filled to the window's edge, behind its text.
    let shown = app.screenshot().foreground().expect("the text is drawn");
    assert!(
        shown.pixels > before.pixels && shown.width > before.width,
        "{shown:?} against {before:?}"
    );

    app.send_keys("C-g");
    assert_eq!(app.state()["windows"][0]["mark_active"], false);
    let hidden = app.screenshot().foreground().expect("the text is drawn");
    assert!(hidden.width < shown.width, "{hidden:?} against {shown:?}");

    app.open_file(&env);
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["secret"], true);
    app.send_keys("C-SPC C-e");
    let state = app.state();
    let window = state["windows"][0]
        .as_object()
        .expect("a window is an object");
    for hidden in ["mark_active", "mark_ring", "selections"] {
        assert!(!window.contains_key(hidden), "{hidden}");
    }
    assert_eq!(state["cursor"], Value::Null);
    app.show_buffer(&notes);
}
