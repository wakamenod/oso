//! `consult-line` in the running app (ADR 0154): `M-s l` lists the buffer's lines from the
//! cursor's on the line searching thread, what is typed narrows them, the preview shows the
//! buffer, `RET` goes to the match with the place before on the mark ring, and beside a buffer
//! that may hold secrets `get-state` shows nothing of the prompt.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The search, its
//! order and its rows have their own tests.

use serde_json::Value;

use super::harness::{App, FAKE_SECRET, Scratch};
use super::{cursor, typing};

/// The rows of the innermost prompt once its search has ended with some.
fn searched(app: &mut App) -> Vec<String> {
    let state = app.wait_for("a search", |state| {
        state["prompts"][0]["completion"]["pending"] == false
            && state["prompts"][0]["completion"]["total"]
                .as_u64()
                .is_some_and(|total| total > 0)
    });
    state["prompts"][0]["completion"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_s_l_lists_the_lines_from_the_cursors_and_ret_goes_to_the_match() {
    let scratch = Scratch::new("consult-line");
    let file = scratch.write(
        "notes.txt",
        b"alpha needle\nbeta\n\ngamma\nlet needle = 1;\n",
    );
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    // The cursor on `gamma`.
    app.send_keys("M-g g 4 RET");
    app.wait_for("the line", |state| cursor(state) == (3, 0));

    app.send_keys("M-s l");
    let rows = searched(&mut app);
    assert_eq!(
        rows,
        [
            "4: gamma",
            "5: let needle = 1;",
            "1: alpha needle",
            "2: beta"
        ]
    );
    let state = app.state();
    assert_eq!(state["prompts"][0]["prompt"], "Go to line:");
    assert_eq!(state["prompts"][0]["completion"]["preview_kind"], "buffer");

    app.send_keys(&typing("needle"));
    let state = app.wait_for("the narrowed rows", |state| {
        state["prompts"][0]["completion"]["pending"] == false
            && state["prompts"][0]["completion"]["total"] == 2
    });
    assert_eq!(
        state["prompts"][0]["completion"]["rows"],
        serde_json::json!(["5: let needle = 1;", "1: alpha needle"])
    );
    app.send_keys("C-n RET");
    let state = app.wait_for("the match", |state| cursor(state) == (0, 6));
    assert!(state["prompts"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(state["cursor"]["mark"]["offset"], 19, "the place before");

    // `C-g` leaves the cursor where it was.
    app.send_keys("M-s l");
    searched(&mut app);
    app.send_keys("C-n C-n C-g");
    let state = app.state();
    assert!(state["prompts"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(cursor(&state), (0, 6));
    let log = app.stop_and_read_log();
    assert!(!log.contains("needle"), "nothing of the search is logged");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn beside_a_buffer_that_may_hold_secrets_nothing_of_the_prompt_shows() {
    let scratch = Scratch::new("consult-line-secret");
    let env = scratch.write(".env", format!("TOKEN={FAKE_SECRET}\nOTHER=1\n").as_bytes());
    let mut app = App::start(Some(&env), scratch.path());
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["secret"], true);
    app.send_keys("M-s l");
    app.send_keys(&typing("OTHER"));
    let state = app.state();
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["prompt"], Value::Null);
    assert_eq!(prompt["completion"], Value::Null);
    app.send_keys("RET");
    app.wait_for("the line", |state| {
        state["prompts"].as_array().is_some_and(Vec::is_empty)
    });
    let log = app.stop_and_read_log();
    assert!(!log.contains(FAKE_SECRET) && !log.contains("[the fake secret]"));
}
