//! Native sessions (ADR 0165, step 1), with a fake `claude -p`: a shell script that `[sessions]
//! claude` names, which writes its arguments and the first line of its input to files, prints
//! stream-json lines the flow wrote, and exits with the status the flow gives it.
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
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/run.$$"
IFS= read -r first
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
        let scratch = Scratch::new(test);
        let project = scratch.path().join("proj");
        fs::create_dir(&project).unwrap();
        git_init(&project);
        fs::write(project.join("notes.md"), "notes\n").unwrap();
        let fake = scratch.write(
            "fake-claude",
            FAKE.replace("DIR", &scratch.path().display().to_string())
                .as_bytes(),
        );
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let config = format!(
            "[terminal]\nshell = \"/bin/sh\"\nlogin = false\n\n[sessions]\nclaude = \"{}\"\nmode = \"native\"\n",
            fake.display()
        );
        let config = scratch.write("config.toml", config.as_bytes());
        scratch.write("stream.jsonl", stream().as_bytes());
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

    // `claude` asks for the first message in `*Brief*`; `C-c C-c` starts the session.
    app.run_command("claude", None).unwrap();
    app.wait_for("*Brief*", |state| state["buffer"]["name"] == "*Brief*");
    app.send_keys(&keys(MESSAGE));
    app.send_keys("C-c C-c");

    let run = flow.wait_for_file("run.");
    let args: Vec<&str> = run.lines().filter_map(|l| l.strip_prefix("arg ")).collect();
    assert_eq!(
        args[..6],
        [
            "-p",
            "--input-format=stream-json",
            "--output-format=stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages"
        ]
    );
    assert!(args.iter().any(|arg| arg.starts_with("--session-id=")));
    assert!(
        args.iter().all(|arg| *arg != "--" && *arg != MESSAGE),
        "{args:?}"
    );
    let first: Value = serde_json::from_str(flow.wait_for_file("first.").trim()).unwrap();
    assert_eq!(first["type"], "user");
    assert_eq!(first["message"]["content"], MESSAGE);

    // The stream's rows, in the session's buffer.
    let state = app.wait_for("the session's rows", |state| {
        state["buffer"]["name"] == "*claude: proj*" && text(state).contains("There is one file.")
    });
    let shown = text(&state);
    assert!(shown.contains(&format!("› {MESSAGE}")), "{shown}");
    assert!(shown.contains("● Reading it."), "{shown}");
    assert!(shown.contains("Bash ls"), "{shown}");
    assert!(shown.contains("1.5 s · $0.0100"), "{shown}");

    // Its keys: `]` goes to the call's row, `TAB` opens it, `RET` shows its whole result,
    // read-only and withheld, and `p` goes back to the message.
    app.send_keys("M-<");
    app.send_keys("]");
    app.wait_for("point on the call", |state| state["cursor"]["row"] == 2);
    app.send_keys("TAB");
    app.wait_for("the call open", |state| {
        text(state).contains("▾ Bash ls\n    notes.md")
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
