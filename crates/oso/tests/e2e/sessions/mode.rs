//! Session mode (ADR 0132, step 1), with a fake `claude` that reads its terminal raw and keeps
//! every byte it gets, so a flow sees exactly what reached the program: `C-c` and a key runs a
//! session command and sends nothing, an unbound one sends nothing either, `C-c C-c` sends one
//! `C-c`, `C-z` is kept back and `C-q C-z` sends it, char mode sends them all, the modeline
//! says `Session`, the buffer is in its directory's project, and after a stop the mode stays and
//! `C-c C-r` resumes (ADR 0143). `[keys.session]` binds and unbinds `C-c` keys there, `C-c ?`
//! says them, and a change to the file takes them away again (step 2).
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the route, the
//! keymaps and the help text have their own tests.

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Flow, asks, ended, shows};
use crate::harness::App;

/// The fake: it reads its terminal raw into `keys`, and exits with the status the flow writes
/// to `go`. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
stty raw -echo
printf 'fake ready\r\n'
# A background command reads /dev/null unless told otherwise.
exec 3<&0
cat <&3 >> "$dir/keys" &
reader=$!
while [ ! -f "$dir/go" ]; do sleep 0.1; done
kill "$reader" 2>/dev/null
exit "$(cat "$dir/go")"
"#;

/// The session's buffer.
const BUFFER: &str = "*claude: proj*";

/// A flow with this file's fake in place of step 1's, in a project: `proj` has a `.git`.
fn flow(test: &str) -> Flow {
    let flow = Flow::new(test);
    let fake = FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(flow.path("fake-claude"), fake).unwrap();
    fs::create_dir(flow.project.join(".git")).unwrap();
    flow
}

/// What reached the program so far.
fn keys(flow: &Flow) -> Vec<u8> {
    fs::read(flow.path("keys")).unwrap_or_default()
}

/// Waits until the program got `wanted`, in all.
fn wait_for_keys(flow: &Flow, app: &mut App, wanted: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while keys(flow) != wanted {
        assert!(
            Instant::now() < deadline,
            "the program got {:?}, not {wanted:?}; the screen: {}",
            String::from_utf8_lossy(&keys(flow)),
            app.state()["terminals"]
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn modeline(state: &Value) -> String {
    state["modeline"].as_str().unwrap_or_default().to_owned()
}

fn echo(state: &Value) -> String {
    state["echo"].as_str().unwrap_or_default().to_owned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_s_c_c_keys_are_oso_s_and_claude_code_gets_the_rest() {
    let flow = flow("sessions-mode");
    let mut app = flow.start();
    app.run_command("claude", None).unwrap();
    let state = app.wait_for("the fake", |state| shows(state, BUFFER, "fake ready"));
    assert!(modeline(&state).contains("Session"), "{}", modeline(&state));

    // An unbound `C-c` key sends nothing, and says so; typing goes on as before.
    app.send_keys("a C-c g");
    app.wait_for("the undefined key", |state| {
        echo(state) == "C-c g is undefined (C-c C-c sends C-c)"
    });
    app.send_keys("b");
    wait_for_keys(&flow, &mut app, b"ab");
    // `C-c C-c` sends one `C-c`.
    app.send_keys("C-c C-c");
    wait_for_keys(&flow, &mut app, b"ab\x03");

    // `C-z` is kept back; `C-q C-z` sends it.
    app.send_keys("C-z");
    app.wait_for("C-z kept", |state| {
        echo(state).starts_with("C-z would suspend Claude Code")
    });
    app.send_keys("C-q C-z c");
    wait_for_keys(&flow, &mut app, b"ab\x03\x1ac");

    // `C-c ?` says the keys.
    app.send_keys("C-c ?");
    app.wait_for("the keys", |state| echo(state).starts_with("C-c then:"));

    // The buffer is in its directory's project: `C-x p f` reads a file of it.
    app.send_keys("C-x p f");
    let state = app.wait_for("the project's files", |state| asks(state).is_some());
    assert!(
        !echo(&state).contains("Not in a project"),
        "{}",
        echo(&state)
    );
    app.send_keys("C-g");
    app.wait_for("the prompt closed", |state| asks(state).is_none());

    // Char mode sends a session key as it is.
    app.send_keys("C-c C-k C-c C-n C-c C-k");
    wait_for_keys(&flow, &mut app, b"ab\x03\x1ac\x03\x0e");

    // `C-c C-p` shows the list and sends nothing (ADR 0143).
    app.send_keys("C-c C-p");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Sessions*"
    });
    app.send_keys("C-c a s");
    app.wait_for("the session again", |state| {
        state["buffer"]["name"] == BUFFER
    });

    // The Agents menu's `K` stops the idle session; the mode stays, and `C-c C-r` resumes it.
    app.send_keys("C-c a K");
    flow.exit_with(0);
    let state = app.wait_for("the stop", |state| ended(state, BUFFER).is_some());
    assert!(modeline(&state).contains("Session"), "{}", modeline(&state));
    flow.hold();
    let _ = fs::remove_file(flow.path("keys"));
    app.send_keys("C-c C-r");
    app.wait_for("the resumed fake", |state| {
        ended(state, BUFFER).is_none() && shows(state, BUFFER, "fake ready")
    });
    app.send_keys("z");
    wait_for_keys(&flow, &mut app, b"z");
    assert_eq!(keys(&flow), b"z", "nothing else from all of that");
    flow.exit_with(0);
    app.wait_for("the end", |state| ended(state, BUFFER).is_some());
}

/// `[keys.session]`, appended to the flow's configuration.
const SESSION_KEYS: &str = "\n[keys.session]\n\"C-c g\" = \"sessions\"\n\"C-c C-p\" = false\n";

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_s_keys_from_the_configuration_apply_and_go_with_it() {
    let flow = flow("sessions-mode-keys");
    let config = fs::read_to_string(&flow.config).unwrap();
    fs::write(&flow.config, format!("{config}{SESSION_KEYS}")).unwrap();
    let mut app = flow.start();
    app.run_command("claude", None).unwrap();
    app.wait_for("the fake", |state| shows(state, BUFFER, "fake ready"));

    // `C-c g` runs what the file binds it to, and `C-c C-p`, which it unbinds, is undefined.
    app.send_keys("C-c g");
    app.wait_for("the list selected", |state| {
        state["buffer"]["name"] == "*Sessions*"
    });
    app.send_keys("C-c a s");
    app.wait_for("the session again", |state| {
        state["buffer"]["name"] == BUFFER
    });
    app.send_keys("C-c C-p");
    app.wait_for("the unbound key", |state| {
        echo(state) == "C-c C-p is undefined (C-c C-c sends C-c)"
    });
    app.send_keys("C-c ?");
    let state = app.wait_for("the keys", |state| echo(state).starts_with("C-c then:"));
    let said = echo(&state);
    assert!(said.contains("  g sessions"), "{said}");
    assert!(!said.contains("  C-p sessions"), "{said}");

    // Without the table, the defaults are back: `C-c ?` says `C-p` again.
    let new = flow.config.with_extension("new");
    fs::write(&new, config).unwrap();
    fs::rename(&new, &flow.config).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.send_keys("C-c ?");
        let state = app.wait_for("the keys", |state| echo(state).starts_with("C-c then:"));
        if echo(&state).contains("  C-p sessions") {
            assert!(!echo(&state).contains("  g sessions"), "{}", echo(&state));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the keys stayed: {}",
            echo(&state)
        );
        thread::sleep(Duration::from_millis(200));
    }

    // None of it reached the program.
    app.send_keys("a");
    wait_for_keys(&flow, &mut app, b"a");
    flow.exit_with(0);
    app.wait_for("the end", |state| ended(state, BUFFER).is_some());
}
