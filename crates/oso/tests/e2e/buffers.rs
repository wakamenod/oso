//! Buffers in the running app (ADR 0088): `C-x b` with its preview following the selection, a
//! modified buffer killed after its question, and `M-,` and `M-/`.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The order, the
//! names and the rules of a kill have their own tests.

use serde_json::Value;

use super::harness::{App, Scratch};

/// The id of the buffer named `name` in `state`.
fn id_of(state: &Value, name: &str) -> Value {
    let buffers = state["buffers"].as_array().expect("buffers are listed");
    let buffer = buffers.iter().find(|buffer| buffer["name"] == name);
    buffer.map_or(Value::Null, |buffer| buffer["id"].clone())
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_b_previews_the_selected_buffer_and_switches_to_it() {
    let scratch = Scratch::new("buffers-switch");
    let plan = scratch.write("plan.md", b"budget\nhiring\n");
    let notes = scratch.write("notes.md", b"agenda\n");
    let mut app = App::start(Some(&plan), scratch.path());
    app.visual_aids_off();
    app.wait_until_ready();
    app.open_file(&notes);
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["name"], "notes.md");
    let before = app.screenshot().foreground().expect("the text is drawn");

    app.send_keys("C-x b");
    let state = app.state();
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["kind"], "complete");
    assert_eq!(prompt["prompt"], "Switch to buffer:");
    let completion = &prompt["completion"];
    assert_eq!(
        completion["rows"][0], "plan.md",
        "the one shown before first"
    );
    assert_eq!(completion["preview"], id_of(&state, "plan.md"));
    let shown = app.screenshot().foreground().expect("the float is drawn");
    assert_ne!(shown.pixels, before.pixels);

    app.send_keys("C-n");
    let state = app.state();
    let completion = &state["prompts"][0]["completion"];
    assert_eq!(completion["selected"], 1);
    assert_ne!(
        completion["preview"],
        id_of(&state, "plan.md"),
        "it follows"
    );

    app.send_keys("C-p RET");
    let state = app.state();
    assert_eq!(state["prompts"], serde_json::json!([]));
    assert_eq!(state["buffer"]["name"], "plan.md");

    // M-, goes back, and M-/ comes back from there.
    app.send_keys("M-,");
    assert_eq!(app.state()["buffer"]["name"], "notes.md");
    app.send_keys("M-/");
    assert_eq!(app.state()["buffer"]["name"], "plan.md");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_x_k_asks_before_killing_a_modified_buffer() {
    let scratch = Scratch::new("buffers-kill");
    let plan = scratch.write("plan.md", b"budget\n");
    let notes = scratch.write("notes.md", b"agenda\n");
    let mut app = App::start(Some(&plan), scratch.path());
    app.wait_until_ready();
    app.open_file(&notes);
    app.wait_until_ready();
    app.send_keys("x C-x k RET");
    let state = app.state();
    let question = &state["prompts"][0];
    assert_eq!(question["kind"], "question");
    let label = question["prompt"].as_str().unwrap_or_default();
    assert!(label.starts_with("notes.md is modified"), "{label}");
    app.send_keys("y");
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "plan.md", "the one shown before");
    assert_eq!(id_of(&state, "notes.md"), Value::Null, "gone");
    assert_eq!(
        std::fs::read_to_string(&notes).unwrap(),
        "agenda\n",
        "nothing was saved"
    );
}
