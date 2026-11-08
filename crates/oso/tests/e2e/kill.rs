//! Kills and yanks through the socket's keys and a clipboard in memory (ADR 0074): a kill
//! reaches the clipboard from its thread, a yank waits for the clipboard and takes what
//! another app copied, and a kill from a buffer that may hold secrets goes there
//! concealed, shows nothing over the socket and makes the buffer it is yanked into secret.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. Pieces,
//! appending and typing over the region are in the dispatcher's scenario tests.

use serde_json::{Value, json};

use super::harness::{App, FAKE_SECRET, Scratch};

fn first_row(state: &Value) -> String {
    state["text"]["rows"][0]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn clipboard(app: &mut App) -> Value {
    app.call("clipboard", None)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn kills_reach_the_clipboard_yanks_read_it_and_secrets_stay_concealed() {
    let scratch = Scratch::new("kill-yank");
    let file = scratch.write("notes.txt", b"one two\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    let state = app.wait_until_ready();
    let notes = state["buffer"]["id"].clone();

    app.send_keys("M-d");
    let state = app.state();
    assert_eq!(first_row(&state), " two");
    assert_eq!(state["kill_ring"]["newest"]["pieces"], json!(["one"]));
    let started = std::time::Instant::now();
    while clipboard(&mut app)["text"] != "one" {
        assert!(
            started.elapsed().as_secs() < 10,
            "the kill never reached the clipboard"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Another app copies; the yank waits for the clipboard, and what follows it waits too.
    app.call("clipboard", Some(json!({ "text": "pasted" })));
    app.send_keys("C-e C-y !");
    let state = app.wait_for("the yank", |state| first_row(state) == " twopasted!");
    assert_eq!(state["kill_ring"]["newest"]["pieces"], json!(["pasted"]));
    assert_eq!(state["clipboard"]["waiting"], false);

    app.open_file(&env);
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["secret"], true);
    app.send_keys("C-k");
    assert_eq!(
        app.state()["kill_ring"],
        Value::Null,
        "nothing while it is shown"
    );
    let started = std::time::Instant::now();
    loop {
        let held = clipboard(&mut app);
        if held["concealed"] == true {
            assert_eq!(held["text"], Value::Null, "never over the socket");
            break;
        }
        assert!(
            started.elapsed().as_secs() < 10,
            "the secret kill never reached the clipboard"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    app.show_buffer(&notes);
    let state = app.state();
    assert_eq!(
        state["kill_ring"]["newest"]["pieces"],
        json!(["pasted"]),
        "the secret one left out"
    );
    assert_eq!(state["buffer"]["secret"], false);
    app.send_keys("C-y");
    let state = app.wait_for("the yank of the secret", |state| {
        state["clipboard"]["waiting"] == false && state["buffer"]["secret"] == true
    });
    assert_eq!(state["text"], Value::Null, "the notes are withheld now");
}
