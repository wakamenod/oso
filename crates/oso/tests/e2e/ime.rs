//! The input method, through the dev socket's stand-in for it (ADR 0069): a preedit
//! drawn and a commit made through the window's path, when oso turns the IME off and
//! on, and what a restart and a secret buffer do with a composition.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. What only
//! the OS's input method does is a manual check.

use serde_json::{Value, json};

use super::harness::{App, FAKE_SECRET, Scratch};
use super::{assert_visible_row, cursor};

fn ime(app: &mut App, params: Value) -> Result<Value, String> {
    app.try_call("ime", Some(params))
}

fn allowed(app: &mut App) -> bool {
    app.state()["ime"]["allowed"] == json!(true)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_preedit_draws_in_the_window_and_a_commit_is_one_undo_step() {
    let scratch = Scratch::new("ime-commit");
    let file = scratch.write("notes.txt", b"abc\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    // The OS gets the cursor's cell once a frame is presented.
    let state = app.wait_for("a cursor area", |state| !state["ime"]["area"].is_null());
    assert_eq!(state["ime"]["allowed"], true);
    assert_eq!(state["ime"]["composing"], false);
    let before = app.screenshot().foreground().expect("the cursor is drawn");

    ime(&mut app, json!({ "preedit": "にほ", "range": [6, 6] })).unwrap();
    let state = app.state();
    assert_eq!(state["ime"]["composing"], true);
    assert_eq!(state["ime"]["preedit"], "にほ");
    assert_eq!(state["ime"]["range"], json!([6, 6]));
    assert_visible_row(&state, 0, "abc");
    assert_eq!(
        state["buffer"]["modified"], false,
        "a preedit is in no buffer"
    );
    // The underline at least, and the kana where text draws, push the box right.
    let composing = app.screenshot().foreground().expect("the cursor is drawn");
    assert!(
        composing.pixels > before.pixels && composing.width > before.width,
        "{composing:?} against {before:?}"
    );
    // The view does not give a restart a composition to lose.
    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(refused.contains("a composition is open"), "{refused}");

    ime(&mut app, json!({ "commit": "日本" })).unwrap();
    let state = app.state();
    assert_eq!(state["ime"]["composing"], false);
    assert_eq!(state["ime"]["preedit"], Value::Null);
    assert_visible_row(&state, 0, "日本abc");
    assert_eq!(cursor(&state), (0, 4));
    // Now what holds a restart is the edit, which a file can carry.
    let refused = app.try_call("check-handover", None).unwrap_err();
    assert!(refused.contains("unsaved edits"), "{refused}");

    app.send_keys("x");
    ime(&mut app, json!({ "commit": "、" })).unwrap();
    assert_visible_row(&app.state(), 0, "日本x、abc");
    app.send_keys("C-z");
    assert_visible_row(&app.state(), 0, "日本xabc");
    app.send_keys("C-z C-z");
    assert_visible_row(&app.state(), 0, "abc");

    // A composition cleared with no commit leaves the text as it was.
    ime(&mut app, json!({ "preedit": "か" })).unwrap();
    ime(&mut app, json!({ "clear": true })).unwrap();
    let state = app.state();
    assert_eq!(state["ime"]["composing"], false);
    assert_visible_row(&state, 0, "abc");
    // `Disabled` from the OS ends one too, and oso still wants the IME.
    ime(&mut app, json!({ "preedit": "か" })).unwrap();
    ime(&mut app, json!({ "disabled": true })).unwrap();
    let state = app.state();
    assert_eq!(state["ime"]["composing"], false);
    assert_eq!(state["ime"]["allowed"], true);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_ime_is_off_where_keys_are_read_as_keys_and_waits_for_a_composition() {
    let scratch = Scratch::new("ime-off");
    let file = scratch.write("notes.txt", b"abc\n");
    let binary = scratch.write("data.bin", b"\xff\xfe\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    let state = app.wait_until_ready();
    let notes = state["buffer"]["id"].clone();
    assert!(allowed(&mut app));

    for (keys, back) in [("C-x", "C-g"), ("C-u", "C-g"), ("C-h r", "q")] {
        app.send_keys(keys);
        let state = app.state();
        assert_eq!(state["ime"]["wanted"], false, "after {keys}");
        assert_eq!(state["ime"]["allowed"], false, "after {keys}");
        let refused = ime(&mut app, json!({ "preedit": "か" })).unwrap_err();
        assert!(refused.contains("the input method is off"), "{refused}");
        app.send_keys(back);
        assert!(allowed(&mut app), "after {keys} {back}");
    }

    // A buffer that refuses text turns it off, but only once the composition ends.
    ime(&mut app, json!({ "preedit": "か" })).unwrap();
    app.open_file(&binary);
    app.wait_until_ready();
    let state = app.state();
    assert_eq!(state["ime"]["wanted"], false);
    assert_eq!(
        state["ime"]["allowed"], true,
        "the OS would keep the composition"
    );
    ime(&mut app, json!({ "clear": true })).unwrap();
    assert!(!allowed(&mut app));

    // A buffer that may hold secrets has it off, and gives no area.
    app.open_file(&env);
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["secret"], true);
    assert_eq!(state["ime"]["allowed"], false);
    assert_eq!(state["ime"]["area"], Value::Null);
    app.show_buffer(&notes);
    assert!(allowed(&mut app));
}
