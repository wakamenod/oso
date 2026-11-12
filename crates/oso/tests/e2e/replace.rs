//! query-replace in the running app (ADR 0113): its two prompts, the count its worker sends,
//! the highlights on screen, the input method off while it reads answers, a restart held
//! while it is on, `!`'s slices replacing the rest, and the whole session undone at once.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The answers, the
//! case and the scope have their own tests.

use serde_json::json;

use super::harness::{App, Scratch};
use super::typing;

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_percent_asks_at_each_match_and_undoes_as_one_step() {
    let scratch = Scratch::new("replace");
    let file = scratch.write("notes.txt", "foo\n".repeat(40).as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let before = app.screenshot().foreground().expect("the text is drawn");

    app.send_keys(&format!("M-% {} RET {} RET", typing("foo"), typing("bar")));
    let state = app.wait_for("the count", |state| !state["replace"]["count"].is_null());
    assert_eq!(
        state["replace"]["count"],
        json!({ "index": 1, "total": 40 })
    );
    assert_eq!(
        state["echo"],
        "Query replacing foo with bar: [1/40] (? for help)"
    );
    assert_eq!(state["ime"]["wanted"], false, "the answers are keys");
    let shown = app.screenshot().foreground().expect("the text is drawn");
    assert_ne!(shown.pixels, before.pixels, "the matches are highlighted");
    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(refused.contains("a search is under way"), "{refused}");

    app.send_keys("y n !");
    let state = app.wait_for("the end", |state| state["replace"].is_null());
    assert_eq!(state["echo"], "Replaced 39 occurrences (skipped 1)");
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 3 })));
    assert_eq!(rows["rows"], json!(["bar", "foo", "bar"]));
    app.call("check-handover", None);

    app.send_keys("C-z");
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 3 })));
    assert_eq!(rows["rows"], json!(["foo", "foo", "foo"]), "one step");
}
