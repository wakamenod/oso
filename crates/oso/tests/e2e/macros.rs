//! Keyboard macros in the running app (ADR 0113): `Def` on the modeline while one is recorded,
//! a replay with a count and an isearch in it, fed a little at each turn of the loop, `e` again,
//! and a restart held while one is recorded.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. What a recording
//! takes and what stops a replay have their own tests.

use serde_json::json;

use super::harness::{App, Scratch};

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_macro_records_with_def_shown_and_replays_with_a_count_and_e() {
    let scratch = Scratch::new("macros");
    let file = scratch.write("words.txt", b"x1 x2 x3 x4 x5\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("C-x (");
    let state = app.state();
    assert_eq!(state["macro"]["recording"], true);
    assert!(
        state["modeline"]
            .as_str()
            .is_some_and(|line| line.contains("Def")),
        "{}",
        state["modeline"]
    );
    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(refused.contains("keyboard macro"), "{refused}");
    app.send_keys("C-s x RET DEL C-x )");
    let state = app.state();
    assert_eq!(state["macro"]["recording"], false);

    app.send_keys("C-u 2 C-x e");
    app.wait_for("the replay's end", |state| {
        state["macro"]["replaying"] == false
    });
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 1 })));
    assert_eq!(rows["rows"], json!(["1 2 3 x4 x5"]));
    app.send_keys("e");
    app.wait_for("the replay's end", |state| {
        state["macro"]["replaying"] == false
    });
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 1 })));
    assert_eq!(rows["rows"], json!(["1 2 3 4 x5"]));
}
