//! Completion in the running app (ADR 0076): `M-x` lists the commands, narrows as a name is
//! typed, moves with `C-n` and `C-p`, and runs what `RET` takes with the prefix argument
//! given before it; a command that reads opens its prompt after. An `M-x` the socket opened
//! records no history.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. How candidates
//! match and are ordered, and how the float lays them out, have their own tests.

use serde_json::json;

use super::harness::{App, Scratch};
use super::{cursor, numbered_lines, typing};

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn m_x_lists_the_commands_narrows_and_runs_the_one_taken() {
    let scratch = Scratch::new("complete-m-x");
    let file = scratch.write("lines.txt", &numbered_lines(60));
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("M-x");
    let state = app.state();
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["kind"], "complete");
    assert_eq!(prompt["prompt"], "M-x");
    let completion = &prompt["completion"];
    assert_eq!(completion["count"], completion["total"], "all listed");
    assert_eq!(completion["selected"], 0);
    assert_eq!(state["ime"]["wanted"], false, "a prompt reads keys");

    app.send_keys(&format!("{} SPC {}", typing("char"), typing("forward")));
    let completion = app.state()["prompts"][0]["completion"].clone();
    assert_eq!(completion["rows"][0], "forward-char");
    app.send_keys("C-n C-p RET");
    let state = app.state();
    assert_eq!(state["prompts"], json!([]));
    assert_eq!(cursor(&state), (0, 1));

    // The prefix argument goes to the command. What the socket ran is not the person's, so
    // it entered no history and does not come first.
    app.send_keys("C-u 3 M-x");
    let prompt = app.state()["prompts"][0].clone();
    assert_eq!(prompt["prompt"], "3 M-x");
    assert_ne!(prompt["completion"]["rows"][0], "forward-char");
    app.send_keys(&typing("forward-char"));
    app.send_keys("RET");
    assert_eq!(cursor(&app.state()), (0, 4));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_command_m_x_runs_that_reads_opens_its_prompt_after() {
    let scratch = Scratch::new("complete-goto-line");
    let file = scratch.write("lines.txt", &numbered_lines(60));
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();

    app.send_keys("M-x");
    app.send_keys(&typing("goto-line"));
    app.send_keys("RET");
    let prompts = app.state()["prompts"].clone();
    assert_eq!(prompts[0]["kind"], "number");
    assert_eq!(prompts[0]["prompt"], "Go to line: ");
    assert_eq!(
        prompts.as_array().map(Vec::len),
        Some(1),
        "M-x closed first"
    );
    app.send_keys("1 2 RET");
    assert_eq!(cursor(&app.state()), (11, 0));

    app.send_keys("M-x");
    app.send_keys(&typing("qqqq"));
    app.send_keys("RET");
    let prompt = app.state()["prompts"][0].clone();
    assert_eq!(prompt["message"], "No match", "it stays");
    app.send_keys("C-g");
    assert_eq!(app.state()["prompts"], json!([]));
}
