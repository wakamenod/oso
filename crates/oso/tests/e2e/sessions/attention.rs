//! Attention to hosted agent sessions (ADR 0121, step 3), with a fake `claude` that prints the
//! recorded screen a flow writes, as the TUI would: the side list, the states the rules find,
//! the modeline's marks, the notices, a stop that asks first, and another program's session
//! read from the registry.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the rules, the
//! tracker, the list's layout and the attention's timing have their own tests.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::{Flow, terminal};
use crate::harness::App;

/// The fake: it prints `screen` each time a flow writes it, cleared first, and exits with
/// the status the flow writes to `go`. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf 'fake ready\n'
while [ ! -f "$dir/go" ]; do
  if [ -f "$dir/screen" ]; then
    printf '\033[2J\033[H'
    cat "$dir/screen"
    rm -f "$dir/screen"
  fi
  sleep 0.1
done
exit "$(cat "$dir/go")"
"#;

const PERMISSION: &str = include_str!("../../../src/sessions/screens/permission-bash.txt");
const WORKING: &str = include_str!("../../../src/sessions/screens/working.txt");
const DONE: &str = include_str!("../../../src/sessions/screens/done.txt");

/// A flow with this file's fake in place of step 1's.
fn flow(test: &str) -> Flow {
    let flow = Flow::new(test);
    let fake = FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(flow.path("fake-claude"), fake).unwrap();
    flow
}

/// Has the fake print `screen`.
fn print(flow: &Flow, screen: &str) {
    let staged = flow.path("screen.new");
    fs::write(&staged, screen).unwrap();
    fs::rename(staged, flow.path("screen")).unwrap();
}

/// The `sessions` part of the dev state.
fn sessions(state: &Value) -> &Value {
    &state["sessions"]
}

/// The state of the session called `name`, as the list says it.
fn state_of(state: &Value, name: &str) -> Option<String> {
    sessions(state)["sessions"]
        .as_array()?
        .iter()
        .find(|session| session["name"] == name)?["state"]
        .as_str()
        .map(str::to_owned)
}

fn modeline(state: &Value) -> String {
    state["modeline"].as_str().unwrap_or_default().to_owned()
}

/// Starts a session and waits for its fake.
fn start(flow: &Flow) -> App {
    let mut app = flow.start();
    app.run_command("claude", None).unwrap();
    app.wait_for("the fake", |state| {
        terminal(state, "*claude: proj*").is_some_and(|terminal| {
            terminal["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row == "fake ready"))
        })
    });
    app
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_side_list_marks_and_notices_follow_a_sessions_state() {
    let flow = flow("sessions-attention");
    let mut app = start(&flow);

    // The list shows on its own beside the session, which keeps the selected window.
    let state = app.wait_for("the side list", |state| {
        sessions(state)["list"]
            .as_str()
            .is_some_and(|list| list.contains("proj"))
    });
    assert_eq!(state["buffer"]["name"], "*claude: proj*");
    let side = sessions(&state)["side"].as_u64().unwrap();
    let window = state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|window| window["id"] == side)
        .unwrap();
    assert_eq!(window["selected"], false);
    assert!(window["columns"].as_u64().unwrap() <= 32, "{window}");

    // A dialog, with the session shown: needs input, with the dialog's question as its recap.
    print(&flow, PERMISSION);
    let state = app.wait_for("needs input", |state| {
        state_of(state, "proj").is_some_and(|s| s.starts_with("needs input (permission)"))
    });
    assert!(modeline(&state).contains("⚠ 1"), "{}", modeline(&state));
    let recap = sessions(&state)["sessions"][0]["recap"].as_str().unwrap();
    assert!(!recap.is_empty());

    // Away from it, its turn ends: done, a notice, and the mark.
    let notes = flow.project.join("notes.txt");
    fs::write(&notes, "notes\n").unwrap();
    app.open_file(&notes);
    print(&flow, WORKING);
    app.wait_for("working", |state| {
        state_of(state, "proj").as_deref() == Some("working")
    });
    let asked = |state: &Value| sessions(state)["asked"].as_u64().unwrap_or(0);
    let before = asked(&app.state());
    print(&flow, DONE);
    let state = app.wait_for("done and told", |state| {
        state_of(state, "proj").as_deref() == Some("done") && asked(state) > before
    });
    assert!(modeline(&state).contains("● 1"), "{}", modeline(&state));

    // `C-x a d` shows it; 2 s in front of the person and it is seen.
    app.send_keys("C-x a d");
    let state = app.wait_for("shown", |state| state["buffer"]["name"] == "*claude: proj*");
    assert!(state["windows"].as_array().unwrap().len() >= 2);
    // A display with no window manager, as Xvfb on CI, may say the window lost the keyboard
    // and never give it back: the window takes it, as a click on it would.
    app.call("focus", Some(json!({ "focused": true })));
    app.wait_for("seen", |state| {
        state_of(state, "proj").as_deref() == Some("idle")
    });

    // `C-x a s` selects the list, then hides it, and it stays hidden.
    app.send_keys("C-x a s");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Sessions*"
    });
    app.send_keys("C-x a s");
    app.wait_for("the list hidden", |state| sessions(state)["side"].is_null());
    print(&flow, WORKING);
    app.wait_for("working again", |state| {
        state_of(state, "proj").as_deref() == Some("working")
    });
    assert!(sessions(&app.state())["side"].is_null());

    stop_asks_first(&mut app);
    super::quit(&mut app);
}

/// `k` on a working session asks; `y` hangs it up, and it is stopped.
fn stop_asks_first(app: &mut App) {
    app.send_keys("C-x a s");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Sessions*"
    });
    // Point to the session's row: the first that starts with a state's mark.
    app.send_keys("n");
    app.send_keys("k");
    let state = app.wait_for("the question", |state| super::asks(state).is_some());
    let question = super::asks(&state).unwrap();
    assert!(question.contains("hang it up"), "{question}");
    app.send_keys("y");
    app.wait_for("stopped", |state| {
        state_of(state, "proj").is_some_and(|s| s == "stopped" || s == "failed")
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn another_programs_session_in_the_registry_is_listed() {
    let flow = flow("sessions-external");
    // A process outside oso that the registry names, started just now.
    let mut outside = spawn(&Spawn {
        executable: &Executable::absolute("/bin/sleep").unwrap(),
        args: &["60".into()],
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        set: &[],
        terminal: None,
    })
    .unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let registry = flow.home.join(".claude").join("sessions");
    fs::create_dir_all(&registry).unwrap();
    fs::write(
        registry.join(format!("{}.json", outside.id())),
        format!(
            r#"{{"pid":{},"startedAt":{now},"name":"elsewhere","kind":"interactive","status":"busy"}}"#,
            outside.id()
        ),
    )
    .unwrap();
    let mut app = start(&flow);
    app.wait_for("the external row", |state| {
        state_of(state, "elsewhere").as_deref() == Some("working")
    });
    let _ = outside.kill();
    let _ = outside.wait();
    app.wait_for("the external row gone", |state| {
        state_of(state, "elsewhere").is_none()
    });
    flow.exit_with(0);
    app.wait_for("the session's end", |state| {
        state_of(state, "proj").as_deref() == Some("stopped")
    });
    super::quit(&mut app);
}
