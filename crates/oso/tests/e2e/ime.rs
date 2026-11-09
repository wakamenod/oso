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
    // The cell goes to the OS when a key moves the cursor, before any frame, so that
    // a composition the next key starts opens its candidate window there.
    let top = state["ime"]["area"]["y"].as_f64().unwrap();
    app.send_keys("C-n");
    let moved = app.state()["ime"]["area"]["y"].as_f64().unwrap();
    assert!(moved > top, "{moved} below {top}");
    app.send_keys("C-p");

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
    // With the composition over, the edit is text a restart carries (ADR 0090).
    assert!(app.call("check-handover", None)["generation"].is_u64());

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
fn the_ime_is_off_where_keys_are_read_as_keys_and_turning_off_keeps_the_text() {
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

    // A prompt turns it off. Only the socket can open one during a composition, as the OS
    // keeps the keys then; both keys come in one request, and the IME is decided after it.
    // On winit, which cannot commit, that waits for the composition to end; oso's own layer
    // commits it where typing goes, here the prompt, then turns the IME off (ADR 0077).
    ime(&mut app, json!({ "preedit": "か" })).unwrap();
    app.send_keys("M-g g");
    let state = app.state();
    assert_eq!(state["ime"]["wanted"], false);
    if cfg!(all(target_os = "macos", feature = "native-window")) {
        assert_eq!(state["ime"]["allowed"], false, "committed, then off");
        assert_eq!(state["ime"]["composing"], false);
        assert_eq!(state["prompts"][0]["text"], "か");
    } else {
        assert_eq!(
            state["ime"]["allowed"], true,
            "the OS would keep the composition"
        );
        ime(&mut app, json!({ "clear": true })).unwrap();
        assert!(!allowed(&mut app));
    }
    app.send_keys("C-g");
    let state = app.state();
    assert_eq!(state["prompts"], json!([]));
    assert_visible_row(&state, 0, "abc");
    assert!(allowed(&mut app));

    // A file that refuses text has it off.
    app.open_file(&binary);
    let state = app.wait_until_ready();
    assert!(!state["buffer"]["read_only"].is_null());
    assert_eq!(state["ime"]["allowed"], false);

    // A request that leaves a buffer that may hold secrets selected ends a
    // composition, so that the IME goes off there at once.
    app.show_buffer(&notes);
    ime(&mut app, json!({ "preedit": "か" })).unwrap();
    app.open_file(&env);
    let state = app.wait_until_ready();
    assert_eq!(state["ime"]["composing"], false);
    assert_eq!(state["ime"]["allowed"], false);
    let refused = ime(&mut app, json!({ "commit": "か" })).unwrap_err();
    assert!(refused.contains("the input method is off"), "{refused}");
    app.show_buffer(&notes);

    // A buffer that may hold secrets has it off, and gives no area.
    app.open_file(&env);
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["secret"], true);
    assert_eq!(state["ime"]["allowed"], false);
    assert_eq!(state["ime"]["area"], Value::Null);
    app.show_buffer(&notes);
    assert!(allowed(&mut app));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn no_input_context_is_given_while_the_ime_is_off() {
    let scratch = Scratch::new("ime-context");
    let file = scratch.write("notes.txt", b"abc\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    let notes = app.wait_until_ready()["buffer"]["id"].clone();
    // oso's own layer gives none while the IME is off, so no text service reads the view
    // then (ADR 0070); winit keeps its own, and the socket reports none.
    // The window applies what the app asks once the app's handler returns, and a request of
    // the socket may come in the same turn, so each is waited for.
    let native = cfg!(all(target_os = "macos", feature = "native-window"));
    let given = |app: &mut App, on: bool, what: &str| {
        let context = if native { json!(on) } else { Value::Null };
        app.wait_for(what, |state| state["ime"]["input_context"] == context);
    };
    given(&mut app, true, "a context at first");
    app.send_keys("C-x");
    given(&mut app, false, "no context after a prefix key");
    app.send_keys("C-g");
    given(&mut app, true, "a context again");
    app.open_file(&env);
    app.wait_until_ready();
    given(
        &mut app,
        false,
        "no context in a buffer that may hold secrets",
    );
    // A request through the socket is no event AppKit dispatched, and for a buffer that may
    // hold secrets the window layer has AppKit read the context again at once; other flips
    // wait for AppKit's update after the next event.
    let current = if native { json!(false) } else { Value::Null };
    app.wait_for("AppKit's current context gone at once", |state| {
        state["ime"]["current_input_context"] == current
    });
    app.show_buffer(&notes);
    given(&mut app, true, "a context back from it");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_composition_waits_where_its_text_would_not_go_in_as_typed() {
    let scratch = Scratch::new("ime-wait");
    let file = scratch.write("notes.txt", b"abc\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    // A prefix key, a prefix argument, or a question: on either layer the IME stays on until
    // the composition ends, so the text takes no argument and answers nothing (ADR 0077).
    // Only the socket brings these during a composition, as the OS keeps the keys then.
    for (keys, back) in [("C-x", "C-g"), ("C-u", "C-g"), ("x C-x C-c", "n")] {
        ime(&mut app, json!({ "preedit": "か" })).unwrap();
        app.send_keys(keys);
        let state = app.state();
        assert_eq!(state["ime"]["wanted"], false, "after {keys}");
        assert_eq!(state["ime"]["allowed"], true, "{keys} waits");
        assert_eq!(state["ime"]["composing"], true, "{keys} keeps it");
        ime(&mut app, json!({ "clear": true })).unwrap();
        assert!(!allowed(&mut app), "off once it ended, after {keys}");
        app.send_keys(back);
        assert!(allowed(&mut app), "after {keys} {back}");
    }
    let state = app.state();
    assert_visible_row(&state, 0, "xabc");
    assert_eq!(state["prompts"], json!([]));
}
