//! The history of what the person sent native sessions (ADR 0165, "The composer's history, as
//! amended"), with the modes' fake: one for two sessions in two spaces, listed by `C-c C-r`, over
//! a real `oso-restart` and in the file; never a secret composer's; and cleared.
//!
//! Must not depend on: the app's internals; the history's rules, its file and the list have
//! their own tests.

use std::fs;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::super::harness::App;
use super::super::super::spaces::switch_space;
use super::super::super::worktrees::keys;
use super::modes::send;
use super::restart::{idle_in_accept_edits, shown_again, transcript_says};
use super::windows::{other_in_front, restarted};
use super::{Flow, in_composer, text};

/// Waits until the composer, selected, holds `wanted`.
fn holds(app: &mut App, wanted: &str) {
    app.wait_for(&format!("the composer holding {wanted:?}"), |state| {
        in_composer(state) && text(state).trim_end() == wanted
    });
}

/// Waits until `*Messages*`, shown, says each of `wanted`, in that order.
fn messages_say(app: &mut App, wanted: &[&str]) {
    app.send_keys(&format!("C-x b {} RET", keys("*Messages*")));
    app.wait_for("the messages", |state| {
        state["buffer"]["name"] == "*Messages*"
    });
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 1000 })));
        let rows = rows["rows"].as_array().cloned().unwrap_or_default();
        let mut left = wanted.iter().peekable();
        for row in rows.iter().filter_map(Value::as_str) {
            left.next_if(|wanted| row.contains(**wanted));
        }
        if left.peek().is_none() {
            return;
        }
        assert!(Instant::now() < end, "no {wanted:?} in {rows:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The file's text, once `done` of it.
fn file(flow: &Flow, project: &std::path::Path, done: impl Fn(&str) -> bool) -> String {
    let file = project.join("state").join("prompt-history");
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let kept = fs::read_to_string(&file).unwrap_or_default();
        if done(&kept) {
            return kept;
        }
        assert!(
            Instant::now() < end,
            "{}: {kept:?}",
            flow.scratch.path().display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn one_history_for_two_sessions_is_walked_listed_kept_over_a_restart_and_cleared() {
    let (flow, mut old) = idle_in_accept_edits("native-prompt-history", None);
    other_in_front(&flow, &mut old);
    old.send_keys("C-c C-o");
    old.send_keys("i");
    old.wait_for("the composer", in_composer);
    // A walk from a draft, through what both were sent and back to it.
    old.send_keys(&keys("draft"));
    old.send_keys("M-p");
    holds(&mut old, "first");
    old.send_keys("M-p");
    holds(&mut old, "one");
    old.send_keys("M-p");
    holds(&mut old, "one");
    old.send_keys("M-n M-n");
    holds(&mut old, "draft");

    // `C-c C-r` lists it; the dev socket is told no row, and takes no screenshot.
    old.send_keys("C-c C-r");
    let state = old.wait_for("the list", |state| {
        state["prompts"][0]["prompt"] == "Past prompt:"
    });
    assert!(state["prompts"][0]["completion"].is_null(), "{state}");
    assert!(!state.to_string().contains("first"), "{state}");
    let refused = old
        .exchange("screenshot", None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(refused.contains("no screenshot"), "{refused}");
    old.send_keys(&keys("one"));
    old.send_keys("RET");
    // At point, after the draft, and not sent.
    holds(&mut old, "draftone");
    assert!(!flow.scratch.path().join("sent.2.2").exists());
    old.send_keys("C-x h C-w");
    holds(&mut old, "");

    let kept = file(&flow, &flow.project, |kept| kept.contains("first"));
    assert!(kept.contains("\"one\""), "{kept}");
    assert!(!kept.contains("draft"), "{kept}");

    // Over a restart, in the other session's composer too.
    let mut new = restarted(&mut old);
    shown_again(&mut new);
    new.send_keys("M-p");
    holds(&mut new, "first");
    new.send_keys("M-n");
    holds(&mut new, "");
    switch_space(&mut new, "proj");
    new.send_keys(&format!("C-x b {} RET", keys("*claude: proj*")));
    transcript_says(&mut new, "Resumed after oso-restart.");
    new.send_keys("i");
    new.wait_for("the composer", in_composer);
    new.send_keys("M-p M-p");
    holds(&mut new, "one");
    new.send_keys("M-n M-n");
    holds(&mut new, "");

    new.run_command("native-prompt-history-clear", None)
        .unwrap();
    // Waited for in the file: the echo area has what came last, as the agent channel's start
    // for the resumed sessions.
    let kept = file(&flow, &flow.project, |kept| !kept.contains("first"));
    assert!(kept.contains("\"version\":1"), "{kept}");
    new.send_keys("M-p");
    messages_say(&mut new, &["Cleared 2 past prompts", "Nothing sent yet"]);
    let _ = new.exchange("exit", None);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_secret_composer_gives_the_history_nothing() {
    let (flow, mut app) = idle_in_accept_edits("native-prompt-history-secret", None);
    let env = flow.scratch.write(".env", b"TOKEN=fixture\n");
    app.open_file(&env);
    app.wait_for("the secret file", |state| state["buffer"]["secret"] == true);
    app.send_keys("C-k");
    app.send_keys(&format!("C-x b {} RET", keys("*claude: proj*")));
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys("C-y");
    app.wait_for("the secret text", |state| {
        in_composer(state) && state["buffer"]["secret"] == true
    });
    app.send_keys("C-c C-c");
    app.send_keys("M-p");
    app.send_keys("C-c C-r");
    let state = app.state();
    assert_eq!(state["prompts"], json!([]), "{state}");
    // The exit writes what waits.
    let _ = app.exchange("exit", None);
    let kept = file(&flow, &flow.project, |kept| kept.contains("\"one\""));
    assert!(!kept.contains("TOKEN"), "{kept}");
    assert!(!flow.scratch.path().join("sent.1.2").exists());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_history_is_in_no_file_once_the_configuration_says_so() {
    let flow = Flow::with("native-prompt-history-off", super::modes::MODES_FAKE, "");
    let config = fs::read_to_string(&flow.config).unwrap();
    fs::write(&flow.config, format!("{config}prompt_history = false\n")).unwrap();
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    super::super::super::spaces::trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        !super::modes::modeline(state).contains("restricted")
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    send(&mut app, "one");
    flow.wait_for_file("sent.1.1");
    app.send_keys("M-p");
    holds(&mut app, "one");
    app.send_keys("M-n");
    holds(&mut app, "");
    std::thread::sleep(Duration::from_millis(1500));
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
    assert!(!flow.project.join("state").join("prompt-history").exists());
}
