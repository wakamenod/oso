//! Secure input in a buffer that may hold secrets, on oso's own macOS window layer
//! (ADR 0073): asked for while one is selected, and never held while one is not. Whether the
//! system holds it also depends on oso being in front with the keyboard, which a test run
//! does not control, so that is a manual check with `ioreg`.
//!
//! Must not depend on: the app's internals; the flow reads `get-state` only.

use serde_json::Value;

use super::harness::{App, FAKE_SECRET, Scratch};

/// What `get-state` says of secure input: wanted, and held by the window layer.
fn secure(state: &Value) -> (bool, bool) {
    let secure = &state["secure_input"];
    let flag = |name: &str| {
        secure[name]
            .as_bool()
            .unwrap_or_else(|| panic!("secure_input.{name} is not a bool"))
    };
    (flag("wanted"), flag("held"))
}

#[test]
#[ignore = "opens windows; run with `just e2e-native`"]
fn secure_input_is_asked_for_only_while_a_secret_buffer_is_selected() {
    let scratch = Scratch::new("secure-input");
    let plain = scratch.write("plain.txt", b"plain text\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());

    let mut app = App::start(Some(&plain), scratch.path());
    let state = app.wait_until_ready();
    let plain_id = state["buffer"]["id"].clone();
    assert_eq!(secure(&state), (false, false), "a plain file");

    app.open_file(&env);
    app.wait_until_ready();
    let state = app.wait_for("secure input asked for", |state| secure(state).0);
    assert!(secure(&state).0);

    app.show_buffer(&plain_id);
    let state = app.wait_for("secure input no longer asked for", |state| !secure(state).0);
    assert_eq!(secure(&state), (false, false), "never held once not wanted");
}
