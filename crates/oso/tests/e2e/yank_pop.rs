//! `M-y` through the socket's keys and a clipboard in memory (ADR 0091): it waits for the
//! clipboard, lists another app's copy first and the kills after it, and the kill picked goes
//! in at every cursor, in place of what the last pick put there.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The rows, the
//! secret ones and the cycling in a prompt are in the editor's and the dispatcher's tests.

use serde_json::{Value, json};

use super::harness::{App, Scratch};

fn rows(state: &Value) -> Vec<String> {
    state["text"]["rows"]
        .as_array()
        .expect("the rows are listed")
        .iter()
        .map(|row| row.as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_y_lists_another_apps_copy_first_and_puts_an_older_kill_at_every_cursor() {
    let scratch = Scratch::new("yank-pop");
    let file = scratch.write("notes.txt", b"alpha beta\nx\nx\nx\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("M-d C-f M-d");
    let started = std::time::Instant::now();
    while app.call("clipboard", None)["text"] != "beta" {
        assert!(
            started.elapsed().as_secs() < 10,
            "the kills never reached the clipboard"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    app.call("clipboard", Some(json!({ "text": "pasted" })));

    app.send_keys("M-< C-n C-SPC C-f C-> C->");
    app.send_keys("M-y");
    let state = app.wait_for("the list", |state| state["prompts"][0].is_object());
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["prompt"], "Yank from kill ring:");
    assert_eq!(prompt["completion"], Value::Null, "its rows are kills");

    // The rows are the copy, then `beta` and `alpha`.
    app.send_keys("C-n C-n RET");
    let state = app.wait_for("the pick", |state| rows(state)[1] == "alpha");
    assert_eq!(rows(&state)[1..4], ["alpha", "alpha", "alpha"]);
    assert_eq!(state["prompts"], json!([]));

    // `alpha` moved to the front, so the second row is the copy.
    app.send_keys("M-y C-n RET");
    let state = app.wait_for("the second pick", |state| rows(state)[1] == "pasted");
    assert_eq!(rows(&state)[..5], [" ", "pasted", "pasted", "pasted", ""]);
}
