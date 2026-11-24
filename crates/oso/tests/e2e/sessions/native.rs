//! Native sessions (ADR 0165, steps 1 and 2), with a fake `claude -p`: a shell script that
//! `[sessions] claude` names, which writes its arguments and the first message of its input
//! to files, leaving the control layer's requests unanswered (step 2c), prints stream-json
//! lines the flow wrote, and exits with the status the flow gives it.
//! The asking fake also asks the person's permission through `oso mcp --permission`, as its
//! `--mcp-config` names it, the way `claude` calls its `--permission-prompt-tool`.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the stream's
//! lines, rows and rendering, the command line and the tracker have their own tests.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::super::harness::{App, Scratch};
use super::super::spaces::trust_restricted;
use super::super::worktrees::keys;

/// The fake: its arguments, its first message, the stream the flow wrote, then the exit the
/// flow writes to `go`. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
while IFS= read -r first; do
  case "$first" in *'"control_request"'*) ;; *) break ;; esac
done
printf '%s\n' "$first" > "$dir/first.$$"
cat "$dir/stream.jsonl"
while [ ! -f "$dir/go" ]; do sleep 0.1; done
exit "$(cat "$dir/go")"
"#;

/// What the person writes as the first message.
const MESSAGE: &str = "hello there";

struct Flow {
    scratch: Scratch,
    project: PathBuf,
    config: PathBuf,
}

impl Flow {
    fn new(test: &str) -> Flow {
        Self::with(test, FAKE, &stream())
    }

    /// A flow whose fake is `fake` and prints `stream` first.
    fn with(test: &str, fake: &str, stream: &str) -> Flow {
        let scratch = Scratch::new(test);
        let project = scratch.path().join("proj");
        fs::create_dir(&project).unwrap();
        git_init(&project);
        fs::write(project.join("notes.md"), "notes\n").unwrap();
        let fake = scratch.write(
            "fake-claude",
            fake.replace("DIR", &scratch.path().display().to_string())
                .as_bytes(),
        );
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let config = format!(
            // No `mode`: native is the default in a trusted project (ADR 0165, part 4e).
            "[terminal]\nshell = \"/bin/sh\"\nlogin = false\n\n[sessions]\nclaude = \"{}\"\n",
            fake.display()
        );
        let config = scratch.write("config.toml", config.as_bytes());
        scratch.write("stream.jsonl", stream.as_bytes());
        Flow {
            scratch,
            project,
            config,
        }
    }

    fn start(&self) -> App {
        let home = self.scratch.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let mut app = App::start_with_config_and_env(
            &self.project,
            &self.config,
            &[("HOME", home.as_os_str())],
        );
        app.wait_until_ready();
        app
    }

    /// The first file whose name starts with `prefix`, once there.
    fn wait_for_file(&self, prefix: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let found = fs::read_dir(self.scratch.path())
                .unwrap()
                .flatten()
                .find(|entry| entry.file_name().to_string_lossy().starts_with(prefix));
            if let Some(entry) = found {
                let text = fs::read_to_string(entry.path()).unwrap_or_default();
                if !text.is_empty() {
                    return text;
                }
            }
            assert!(Instant::now() < deadline, "no {prefix}");
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// `git init` in `dir`, so that it is a project oso finds.
fn git_init(dir: &std::path::Path) {
    let git = Executable::from_path("git", None).expect("git is on PATH");
    let args = ["init".into(), "-q".into()];
    let child = spawn(&Spawn {
        executable: &git,
        args: &args,
        cwd: Some(dir),
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: false,
        set: &[("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null"))],
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
}

/// What the fake prints: the person's message replayed, a reply, a call and its result, and
/// the turn's end.
fn stream() -> String {
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "text", "text": "Reading it."},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
        ]}}),
        json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "notes.md"},
        ]}}),
        json!({"type": "assistant", "message": {"id": "m2", "content": [
            {"type": "text", "text": "There is one file."},
        ]}}),
        json!({"type": "result", "subtype": "success", "duration_ms": 1500, "total_cost_usd": 0.01}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

/// The text the selected window shows, its view rows one after the other, so that a wrapped
/// line reads whole.
fn joined(state: &Value) -> String {
    (state["text"]["rows"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect()
}

fn text(state: &Value) -> String {
    state["text"]["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Whether a line's one `--settings` turns Remote Control off, as each native one's does
/// (ADR 0165).
pub(super) fn remote_control_off<S: AsRef<str>>(args: &[S]) -> bool {
    let settings: Vec<&str> = (args.iter())
        .filter_map(|arg| arg.as_ref().strip_prefix("--settings="))
        .collect();
    match settings[..] {
        [one] => serde_json::from_str::<Value>(one)
            .is_ok_and(|settings| settings["disableRemoteControl"] == true),
        _ => false,
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_native_session_starts_with_its_first_message_and_shows_its_stream() {
    let flow = Flow::new("native-session");
    let mut app = flow.start();
    app.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });

    // `claude` starts the session at once, with its composer selected; `C-c C-c` sends the
    // first message.
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");
    // Back to the transcript, whose rows the flow reads.
    app.send_keys("C-c C-o");

    let run = flow.wait_for_file("run.");
    let args: Vec<&str> = run.lines().filter_map(|l| l.strip_prefix("arg ")).collect();
    assert_eq!(
        args[..7],
        [
            "-p",
            "--input-format=stream-json",
            "--output-format=stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
            "--prompt-suggestions=true"
        ]
    );
    assert!(args.iter().any(|arg| arg.starts_with("--session-id=")));
    assert!(
        args.iter().all(|arg| *arg != "--" && *arg != MESSAGE),
        "{args:?}"
    );
    assert!(remote_control_off(&args), "{args:?}");
    let first: Value = serde_json::from_str(flow.wait_for_file("first.").trim()).unwrap();
    assert_eq!(first["type"], "user");
    assert_eq!(first["message"]["content"], MESSAGE);

    // The stream's rows, in the session's buffer.
    let state = app.wait_for("the session's rows", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("There is one file.")
    });
    let shown = text(&state);
    assert!(shown.contains(&format!("〉 {MESSAGE}")), "{shown}");
    assert!(shown.contains("\n\n  Reading it.\n"), "{shown}");
    assert!(shown.contains("$ Bash · ls"), "{shown}");
    assert!(shown.contains("1.5 s · $0.0100"), "{shown}");

    // Its keys: `]` goes to the call's row, `TAB` opens it, `RET` shows its whole result,
    // read-only and withheld, and `p` goes back to the message.
    app.send_keys("M-<");
    app.send_keys("]");
    app.wait_for("point on the call", |state| state["cursor"]["row"] == 4);
    app.send_keys("TAB");
    app.wait_for("the call open", |state| {
        text(state).contains("▾ $ Bash · ls\n    notes.md")
    });
    app.send_keys("RET");
    let state = app.wait_for("the result", |state| {
        state["buffer"]["name"] == "*result: proj*"
    });
    assert!(text(&state).starts_with("Bash ls\n\nnotes.md"), "{state}");
    app.send_keys("C-x b");
    app.send_keys(&keys("*claude: proj*"));
    app.send_keys("RET");
    app.wait_for("the session again", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.send_keys("p");
    app.wait_for("point on the message", |state| state["cursor"]["row"] == 0);

    // Its process ends: the session is stopped, and the modeline says so.
    flow.scratch.write("go", b"0");
    app.wait_for("the session stopped", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("stopped"))
    });
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}

/// The asking fake: as [`FAKE`], then, once the flow writes `ask`, the bridge its
/// `--mcp-config` names, as `oso mcp --pid PID --permission`, asked about three calls: the
/// first two the stream has, and one it has not. The bridge's answers go to `answers`; then it
/// prints `after.jsonl` and waits for `go`.
const ASKING_FAKE: &str = r#"#!/bin/sh
dir='DIR'
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
while IFS= read -r first; do
  case "$first" in *'"control_request"'*) ;; *) break ;; esac
done
printf '%s\n' "$first" > "$dir/first.$$"
cat "$dir/stream.jsonl"
while [ ! -f "$dir/ask" ]; do sleep 0.1; done
oso=$(sed -n 's/.*"command":"\([^"]*\)".*/\1/p' "$dir/run.$$" | head -n 1)
pid=$(sed -n 's/.*"--pid","\([0-9]*\)".*/\1/p' "$dir/run.$$" | head -n 1)
ask() {
  printf '{"jsonrpc":"2.0","id":%s,"method":"tools/call","params":{"name":"permission","arguments":{"tool_name":"Bash","input":{"command":"%s"},"tool_use_id":"%s"}}}\n' "$1" "$2" "$3"
}
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
  ask 2 'rm -rf build' t1
  ask 3 'make' t2
  ask 4 'make' t9
  while [ ! -f "$dir/answers" ]; do sleep 0.1; done
} | "$oso" mcp --pid "$pid" --permission | {
  IFS= read -r init
  : > "$dir/partial"
  for n in 2 3 4; do IFS= read -r line; printf '%s\n' "$line" >> "$dir/partial"; done
  mv "$dir/partial" "$dir/answers"
}
cat "$dir/after.jsonl"
while [ ! -f "$dir/go" ]; do sleep 0.1; done
exit "$(cat "$dir/go")"
"#;

/// What the asking fake prints first: the person's message replayed and two calls that wait.
fn asking_stream() -> String {
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "user", "isReplay": true, "message": {"content": MESSAGE}}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "rm -rf build"}},
            {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "make"}},
        ]}}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

/// `flow`'s app, started in its trusted project, with a native session sent [`MESSAGE`] and
/// its transcript selected.
fn sent(flow: &Flow) -> App {
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
    // Back to the transcript, whose rows the flow reads.
    app.send_keys("C-c C-o");
    app
}

/// The card that asks about a call, once point came to it: its text.
fn card(app: &mut App, what: &str) -> String {
    let state = app.wait_for(what, |state| state["buffer"]["name"] == "*card: proj*");
    text(&state)
}

/// The card that says `what_it_says`, once point came to it: one that follows another at once has
/// its place and name.
fn card_saying(app: &mut App, what: &str, what_it_says: &str) -> String {
    let state = app.wait_for(what, |state| {
        state["buffer"]["name"] == "*card: proj*" && text(state).contains(what_it_says)
    });
    text(&state)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
#[expect(
    clippy::disallowed_macros,
    reason = "the flow prints how soon each question opened, for the PR"
)]
fn a_native_sessions_calls_are_asked_of_the_person_with_the_agent_channel_off() {
    let flow = Flow::with("native-asks", ASKING_FAKE, &asking_stream());
    let after = [
        json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "removed"},
            {"type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": "The person denied this call in oso."},
        ]}}),
        json!({"type": "result", "subtype": "success", "duration_ms": 900}),
    ];
    let after: Vec<String> = after.iter().map(Value::to_string).collect();
    flow.scratch
        .write("after.jsonl", format!("{}\n", after.join("\n")).as_bytes());
    let mut app = sent(&flow);

    // The prompt tool and the bridge that lists it are on the session's line.
    let run = flow.wait_for_file("run.");
    assert!(
        run.lines()
            .any(|line| line == "arg --permission-prompt-tool=mcp__oso__permission"),
        "{run}"
    );
    assert!(run.contains(r#""--permission"]"#), "{run}");
    app.wait_for("the calls", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("$ Bash · make")
    });

    // The agent channel goes off; the session still asks.
    app.run_command("agent-channel-stop", None).unwrap();
    app.wait_for("the channel off", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.starts_with("Agent  "))
    });
    flow.scratch.write("ask", b"");

    // Each call the stream has is asked in turn, on the session's card, from what the stream
    // says of it; point goes there as it appears.
    let first = card(&mut app, "the first card");
    assert!(
        first.starts_with("╭─ Bash wants to run, in session proj "),
        "{first}"
    );
    assert!(first.contains("│ $ rm -rf build"), "{first}");
    assert!(first.contains("╰─  y  allow"), "{first}");
    answer_after_quiet(&mut app, "y");
    // Answered, the card goes, and the call that waits behind it has its own, with point there as
    // it appears (ADR 0180).
    card_saying(&mut app, "the second card", "│ $ make");
    // `RET` shows the whole call in the card, and goes back.
    answer_after_quiet(&mut app, "RET");
    app.wait_for("the whole call", |state| {
        text(state).contains(r#""command": "make""#)
    });
    answer_after_quiet(&mut app, "n");

    // `y` gives the CLI its own input back; `n` and a call the stream lacks deny.
    check_answers(&flow);
    assert!(
        !app.state()["windows"]
            .as_array()
            .is_some_and(|windows| windows.iter().any(|w| w["docked"]["slot"] == "card")),
        "no card is docked once answered"
    );

    flow.scratch.write("go", b"0");
    // Point is in the composer; the transcript's modeline says the session stopped.
    app.send_keys("C-c C-o");
    let state = app.wait_for("the session stopped", |state| {
        state["buffer"]["name"] == "*claude: proj*"
            && state["modeline"]
                .as_str()
                .is_some_and(|modeline| modeline.contains("stopped"))
    });
    assert!(!text(&state).contains("waiting for you"), "{state}");
    // How soon each card showed after its request, for the PR (ADR 0165, step 2e-2).
    let log = app.stop_and_read_log();
    for row in log
        .lines()
        .filter(|row| row.contains("permission card shown"))
    {
        println!("{row}");
    }
}

/// What the bridge told the asking fake: an allow with the call's own input, the person's
/// denial, and a denial of the call the stream lacks.
fn check_answers(flow: &Flow) {
    let answers: Vec<Value> = flow
        .wait_for_file("answers")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let said: Vec<Value> = answers
        .iter()
        .map(|answer| {
            let text = answer["result"]["content"][0]["text"].as_str().unwrap();
            serde_json::from_str(text).unwrap()
        })
        .collect();
    assert_eq!(
        said[0],
        json!({"behavior": "allow", "updatedInput": {"command": "rm -rf build"}})
    );
    assert_eq!(said[1]["behavior"], "deny");
    assert_eq!(said[1]["message"], "The person denied this call in oso.");
    assert_eq!(said[2]["behavior"], "deny");
    assert!(
        said[2]["message"]
            .as_str()
            .unwrap()
            .contains("no such call"),
        "{}",
        said[2]
    );
}

/// Waits until the selected card takes an answer: the app's quiet moment has passed since the
/// last key, or point's coming to it.
fn wait_for_answers(app: &mut App) {
    app.wait_for("the card to take an answer", |state| {
        state["card"]["takes_answer"] == true
    });
}

/// Sends `key` once the card takes an answer, again while the card says to wait, as the person
/// would. On X11, winit holds its report that oso turned the IME off until the next X event,
/// which may be the one of the session's attention, asked for as the card comes to take
/// answers: the report counts as typing, and restarts the quiet moment between the look and
/// the key. A key refused does nothing, and every key empties the echo area first, so what it
/// says is of this key.
fn answer_after_quiet(app: &mut App, key: &str) {
    for _ in 0..3 {
        wait_for_answers(app);
        app.send_keys(key);
        if app.state()["echo"] != "Wait a moment, then answer the card" {
            return;
        }
    }
    panic!("the card took no {key}, sent three times as it took answers");
}

mod adopt;
mod card_focus;
mod cards;
mod carried;
mod compose;
mod composed;
mod follow;
mod front;
mod interrupt;
mod markdown;
mod model;
mod modes;
mod past;
mod plan;
mod quit;
mod restart;
mod restart_wait;
mod resume;
mod slash;
mod space;
mod split;
mod status;
mod suggest;
mod switch;
mod todo;
mod wrap;

/// The selected window, if it is the composer docked under a session.
fn selected_composer(state: &Value) -> Option<&Value> {
    (state["windows"].as_array().into_iter().flatten())
        .find(|window| window["selected"] == true && window["docked"]["slot"] == "composer")
}

/// Whether the selected window is the composer docked under a session.
fn in_composer(state: &serde_json::Value) -> bool {
    selected_composer(state).is_some()
}

/// Whether the composer is selected, docked under its session's transcript and laid out, a row
/// or more, inside a window that shows that transcript (ADR 0170): as a session started
/// natively shows it, also after a switch or a restart.
fn composer_docked(state: &Value) -> bool {
    let Some(composer) = selected_composer(state) else {
        return false;
    };
    let windows = state["windows"].as_array().map_or(&[][..], Vec::as_slice);
    let at = |window: &Value, key: &str| window["area"][key].as_u64().unwrap_or(0);
    let (row, rows) = (at(composer, "row"), at(composer, "rows"));
    rows >= 1
        && windows.iter().any(|host| {
            host["docked"].is_null()
                && host["buffer"] == composer["docked"]["host"]
                && at(host, "column") == at(composer, "column")
                && at(host, "row") <= row
                && row + rows <= at(host, "row") + at(host, "rows")
        })
}

/// The tracker's state of the flow's one hosted session, as the dev state gives it: `working`
/// from the replay of what was sent until the turn's result. oso ends the composer's turn as
/// it reads the result, before it tells the tracker, so past `working` a send goes at once.
fn tracked(state: &Value) -> String {
    state["sessions"]["sessions"]
        .as_array()
        .and_then(|sessions| {
            sessions
                .iter()
                .find(|session| session["key"].get("hosted").is_some())
        })
        .and_then(|session| session["state"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Ends the turn the fake holds, by writing `release`: waits until the tracker says the turn
/// works first, as it starts `idle`, so that its end is this turn's, not an earlier state.
fn end_turn(flow: &Flow, app: &mut App, release: &str) {
    app.wait_for("the turn under way", |state| tracked(state) == "working");
    flow.scratch.write(release, b"");
    app.wait_for("the turn's end", |state| {
        let tracked = tracked(state);
        !tracked.is_empty() && tracked != "working"
    });
}
