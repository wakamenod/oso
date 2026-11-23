//! `session-switch-mode` (ADR 0165, step 4, part 4d), with one fake `claude` that serves both
//! modes: given `-p`, it is a stream fake that answers each message at once; else it is a TUI
//! fake that shows Claude Code's idle screen until its terminal hangs up. A native session
//! handed to the TUI ends its `claude -p` first, and the TUI resumes the same conversation,
//! with the mode it held, while its transcript is set aside.
//!
//! Must not depend on: the app's internals. Driven by its control socket and the fake's pty
//! alone; no real key event is posted.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::super::harness::emergency_dir;
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::{Flow, MESSAGE, in_composer, text};

/// The fake: run `N` from 1 logs its arguments to `run.N`. With `-p`, it prints `stream.jsonl`,
/// then replays each message and replies `reply N.M` at once, its line's id `aNM`; once its input ends it takes 2 s
/// more, `SIGTERM` ignored, and writes `ended.N` as it exits; it writes `exits.N` first, how
/// many TUIs had exited by its start. Else it is the TUI: it writes `order.N`, `after` if
/// `ended.1` was there as it started, and shows `idle.txt` until it is hung up, or the flow
/// writes `quit`; hung up while `stubborn` is there, it lets its terminal go and lives 3 s
/// more, past the terminal's 2 s wait for its child. It writes
/// `exited.N` as it exits, with status 0.
const SWITCH_FAKE: &str = r#"#!/bin/sh
dir='DIR'
n=$(ls "$dir" | grep -c '^run\.')
n=$((n+1))
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$n"
mv "$dir/tmp.$n" "$dir/run.$n"
if [ "$1" = -p ]; then
  ls "$dir" | grep -c '^exited\.' > "$dir/exits.$n"
  trap '' TERM
  cat "$dir/stream.jsonl"
  m=0
  while IFS= read -r line; do
    case "$line" in *'"control_request"'*) continue ;; esac
    m=$((m+1))
    said=$(printf '%s\n' "$line" | sed 's/.*"content":"\([^"]*\)".*/\1/')
    printf '{"type":"user","isReplay":true,"message":{"content":"%s"}}\n' "$said"
    printf '{"type":"assistant","uuid":"a%s%s","message":{"id":"m%s%s","content":[{"type":"text","text":"reply %s.%s"}]}}\n' "$n" "$m" "$n" "$m" "$n" "$m"
    printf '{"type":"result","subtype":"success","duration_ms":10}\n'
  done
  sleep 2
  printf 'ended\n' > "$dir/ended.$n"
  exit 0
fi
if [ -f "$dir/ended.1" ]; then echo after; else echo before; fi > "$dir/order.$n"
h=
trap 'h=1' HUP
printf '\033[2J\033[H'
cat "$dir/idle.txt"
while :; do
  sleep 0.1
  if [ -f "$dir/quit" ]; then
    printf 'exited\n' > "$dir/exited.$n"
    exit 0
  fi
  if [ -n "$h" ]; then
    if [ -f "$dir/stubborn" ]; then
      exec </dev/null >/dev/null 2>&1
      sleep 3
    fi
    printf 'exited\n' > "$dir/exited.$n"
    exit 0
  fi
done
"#;

/// Claude Code's idle screen, as the TUI fake shows it.
pub(super) const IDLE: &str = include_str!("../../../../src/sessions/screens/idle-start.txt");

/// A flow whose fake is [`SWITCH_FAKE`], whose stream says the session is in `plan` mode,
/// which a resume carries.
pub(super) fn flow(test: &str) -> Flow {
    let flow = Flow::new(test);
    let script = SWITCH_FAKE.replace("DIR", &flow.scratch.path().display().to_string());
    fs::write(flow.scratch.path().join("fake-claude"), script).unwrap();
    let init = r#"{"type":"system","subtype":"init","cwd":"/w","session_id":"s","model":"m","permissionMode":"plan"}"#;
    flow.scratch
        .write("stream.jsonl", format!("{init}\n").as_bytes());
    flow.scratch.write("idle.txt", IDLE.as_bytes());
    flow
}

/// The arguments run `n` logged.
pub(super) fn args(flow: &Flow, n: usize) -> Vec<String> {
    let run = flow.wait_for_file(&format!("run.{n}"));
    run.lines()
        .filter_map(|line| line.strip_prefix("arg "))
        .map(str::to_owned)
        .collect()
}

/// The value of `--NAME=` in `args`.
pub(super) fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let prefix = format!("--{name}=");
    args.iter()
        .find_map(|arg| arg.strip_prefix(prefix.as_str()))
}

/// `flow`'s app with a native session sent [`MESSAGE`] and answered, its transcript selected.
fn answered(flow: &Flow) -> super::super::super::harness::App {
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");
    app.send_keys("C-c C-o");
    app.wait_for("the reply", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("reply 1.1")
    });
    app
}

/// The record's sessions, once one holds what `holds` looks for.
fn wait_for_record(project: &Path, what: &str, holds: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let place = emergency_dir(project).parent().unwrap().join("sessions");
    let started = Instant::now();
    loop {
        let sessions: Vec<Value> = fs::read_to_string(&place)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        if holds(&sessions) {
            return sessions;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "no {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The rows of every terminal the state has.
pub(super) fn screen(state: &Value) -> String {
    (state["terminals"].as_array().into_iter().flatten())
        .flat_map(|terminal| terminal["rows"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sends `key`, which the transcript set aside, or its composer, refuses, and waits for the
/// echo area to say so.
fn refused(app: &mut super::super::super::harness::App, key: &str) {
    app.run_command("keyboard-quit", None).unwrap();
    app.send_keys(key);
    app.wait_for(&format!("{key} refused"), |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("This session runs in the TUI now"))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_session_is_handed_to_the_tui_which_resumes_its_conversation() {
    let flow = flow("native-switch-tui");
    let mut app = answered(&flow);
    let native = args(&flow, 1);
    let id = flag(&native, "session-id")
        .expect("a new session's id")
        .to_owned();
    wait_for_record(&flow.project, "the native session", |sessions| {
        sessions.iter().any(|s| s["native"] == true)
    });

    // Handed to the TUI: `claude -p` ends first, then the TUI resumes the same conversation
    // in the mode the session held.
    app.run_command("session-switch-mode", None).unwrap();
    flow.wait_for_file("ended.1");
    let tui = args(&flow, 2);
    // Never two writers: the TUI started only once `claude -p` had exited.
    assert_eq!(flow.wait_for_file("order.2").trim(), "after");
    assert!(!tui.iter().any(|arg| arg == "-p"), "{tui:?}");
    assert_eq!(flag(&tui, "resume"), Some(id.as_str()), "{tui:?}");
    assert_eq!(flag(&tui, "permission-mode"), Some("plan"), "{tui:?}");
    assert_eq!(flag(&tui, "permission-prompt-tool"), None, "{tui:?}");
    // The terminal takes the session's name, and shows the TUI.
    app.wait_for("the TUI's screen", |state| {
        state["buffer"]["name"] == "*claude: proj*" && screen(state).contains("Claude Code")
    });
    // The record says it is a TUI's.
    wait_for_record(&flow.project, "the TUI session", |sessions| {
        sessions.len() == 1 && sessions[0]["native"].is_null()
    });

    // The transcript is set aside, its rows kept, and sends nothing.
    app.send_keys("C-x b");
    app.send_keys(&keys("*native: proj*"));
    app.send_keys("RET");
    let state = app.wait_for("the transcript set aside", |state| {
        state["buffer"]["name"] == "*native: proj*"
    });
    let shown = text(&state);
    assert!(shown.contains("reply 1.1"), "{shown}");
    assert!(shown.contains("In the TUI since here"), "{shown}");
    for key in ["i", "S-TAB", "C-c C-k"] {
        refused(&mut app, key);
    }
    // No third run: nothing else started.
    std::thread::sleep(Duration::from_millis(500));
    assert!(!flow.scratch.path().join("run.3").exists());
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_transcript_killed_while_its_claude_ends_for_the_tui_starts_nothing() {
    let flow = flow("native-switch-killed");
    let mut app = answered(&flow);
    // Its `claude -p` takes 2 s to end: the transcript is killed meanwhile.
    app.run_command("session-switch-mode", None).unwrap();
    // It runs a program: the kill asks first.
    app.send_keys("C-x k RET");
    app.wait_for("the kill's question", |state| {
        state["prompts"].as_array().is_some_and(|p| !p.is_empty())
    });
    app.send_keys("y");
    flow.wait_for_file("ended.1");
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !flow.scratch.path().join("run.2").exists(),
        "no TUI started"
    );
    wait_for_record(&flow.project, "the session archived", |sessions| {
        sessions.len() == 1 && sessions[0]["archived"] == true && sessions[0]["native"] == true
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_session_stopped_while_its_claude_ends_for_the_tui_stays_native() {
    let flow = flow("native-switch-stopped");
    let mut app = answered(&flow);
    // Its `claude -p` takes 2 s to end: the person stops the session meanwhile.
    app.run_command("session-switch-mode", None).unwrap();
    app.run_command("session-stop", None).unwrap();
    flow.wait_for_file("ended.1");
    app.wait_for("the switch given up", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("The TUI did not start")
    });
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !flow.scratch.path().join("run.2").exists(),
        "no TUI started"
    );
    wait_for_record(&flow.project, "the session native", |sessions| {
        sessions.len() == 1 && sessions[0]["native"] == true && sessions[0]["archived"] != true
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// `flow`'s app with its native session handed to the TUI, whose screen says it is idle, its
/// terminal selected; and the session's id.
pub(super) fn in_the_tui(flow: &Flow) -> (super::super::super::harness::App, String) {
    let mut app = answered(flow);
    let id = flag(&args(flow, 1), "session-id").unwrap().to_owned();
    app.run_command("session-switch-mode", None).unwrap();
    assert_eq!(flag(&args(flow, 2), "resume"), Some(id.as_str()));
    tui_idle(&mut app);
    (app, id)
}

/// Waits until `app` shows the session's TUI, `*claude: proj*`, whose screen the tracker
/// judged idle.
pub(super) fn tui_idle(app: &mut super::super::super::harness::App) {
    app.wait_for("the TUI idle", |state| {
        state["buffer"]["name"] == "*claude: proj*"
            && screen(state).contains("Claude Code")
            && super::tracked(state) == "idle"
    });
}

/// Whether the session is native again in its own transcript, `*claude: NAME*`, its rows
/// kept, and no transcript is set aside.
fn back_natively(state: &Value) -> bool {
    let set_aside = (state["buffers"].as_array().into_iter().flatten())
        .any(|buffer| buffer["name"] == "*native: proj*");
    state["buffer"]["name"] == "*claude: proj*"
        && !set_aside
        && text(state).contains("reply 1.1")
        && text(state).contains("Back from the TUI, in mode plan.")
}

/// The session, resumed natively, selects its composer: back in its transcript, which is the
/// one set aside, taken back.
pub(super) fn taken_back(app: &mut super::super::super::harness::App) {
    app.wait_for("the composer", in_composer);
    app.send_keys("C-c C-o");
    app.wait_for("the transcript taken back", back_natively);
}

/// `C-c C-t` in the TUI's terminal, again until the tracker has judged the TUI's screen: until
/// then its state is not known, and the switch says so.
pub(super) fn switch_back(app: &mut super::super::super::harness::App) {
    let started = Instant::now();
    loop {
        app.send_keys("C-c C-t");
        let state = app.wait_for("the switch's answer", |state| {
            state["echo"].as_str().is_some_and(|echo| {
                echo.contains("comes back natively") || echo.contains("not known yet")
            })
        });
        if state["echo"]
            .as_str()
            .is_some_and(|e| e.contains("comes back natively"))
        {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "{state}");
        app.run_command("keyboard-quit", None).unwrap();
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_tui_session_is_taken_back_natively_once_its_claude_is_gone() {
    let flow = flow("native-switch-back");
    let (mut app, id) = in_the_tui(&flow);
    // A TUI that outlives its hang-up by 3 s, past the terminal's own 2 s wait for its child.
    flow.scratch.write("stubborn", b"");
    switch_back(&mut app);
    let native = args(&flow, 3);
    assert_eq!(native.first().map(String::as_str), Some("-p"), "{native:?}");
    assert_eq!(flag(&native, "resume"), Some(id.as_str()), "{native:?}");
    // Never two writers: `claude -p` started only once the TUI had exited.
    assert_eq!(flow.wait_for_file("exits.3").trim(), "1");
    assert!(flow.scratch.path().join("exited.2").exists());
    taken_back(&mut app);
    wait_for_record(&flow.project, "the native session", |sessions| {
        sessions.len() == 1 && sessions[0]["native"] == true
    });
    // It runs natively again: a message gets its reply.
    app.send_keys("i");
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("again"));
    app.send_keys("C-c C-c");
    app.send_keys("C-c C-o");
    app.wait_for("the reply", |state| text(state).contains("reply 3.1"));
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_tui_the_hand_off_started_that_exits_of_itself_comes_back_natively() {
    let flow = flow("native-switch-exit");
    let (mut app, id) = in_the_tui(&flow);
    // `/exit` in the TUI: it ends with status 0 of itself.
    flow.scratch.write("quit", b"");
    let native = args(&flow, 3);
    assert_eq!(native.first().map(String::as_str), Some("-p"), "{native:?}");
    assert_eq!(flag(&native, "resume"), Some(id.as_str()), "{native:?}");
    assert_eq!(flow.wait_for_file("exits.3").trim(), "1");
    taken_back(&mut app);
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_tui_stopped_before_its_claude_ended_is_taken_back_only_once_it_is_gone() {
    let flow = flow("native-switch-stopped-tui");
    let (mut app, id) = in_the_tui(&flow);
    // Stopped: its terminal gives up on its child 2 s after the hang-up, a second before it
    // ends, and says it stopped.
    flow.scratch.write("stubborn", b"");
    app.run_command("session-stop", None).unwrap();
    app.wait_for("the TUI stopped", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("stopped"))
    });
    // Shown stopped while its `claude` still runs, what the native start must wait for: on
    // Linux, whose pty ends once its last descriptor closes. On macOS a session leader's
    // controlling terminal stays open until it exits, so the terminal's end waits for it.
    if cfg!(target_os = "linux") {
        assert!(
            !flow.scratch.path().join("exited.2").exists(),
            "the TUI ended already"
        );
    }
    app.send_keys("C-c C-t");
    let native = args(&flow, 3);
    assert_eq!(flag(&native, "resume"), Some(id.as_str()), "{native:?}");
    // Never two writers: the native `claude -p` started once the TUI's had exited.
    assert_eq!(flow.wait_for_file("exits.3").trim(), "1");
    taken_back(&mut app);
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_tui_killed_while_it_ends_for_native_starts_nothing() {
    let flow = flow("native-switch-tui-killed");
    let (mut app, _) = in_the_tui(&flow);
    flow.scratch.write("stubborn", b"");
    switch_back(&mut app);
    // Its buffer is killed while its `claude` takes 3 s to end; the kill asks first while its
    // terminal still runs.
    app.send_keys("C-x k RET");
    let state = app.wait_for("the kill", |state| {
        state["prompts"].as_array().is_some_and(|p| !p.is_empty())
            || state["buffer"]["name"] != "*claude: proj*"
    });
    if state["prompts"].as_array().is_some_and(|p| !p.is_empty()) {
        app.send_keys("y");
    }
    flow.wait_for_file("exited.2");
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !flow.scratch.path().join("run.3").exists(),
        "no native start"
    );
    wait_for_record(&flow.project, "the session archived", |sessions| {
        sessions.len() == 1 && sessions[0]["archived"] == true
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
