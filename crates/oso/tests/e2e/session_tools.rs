//! The agents' session tools (ADR 0121, step 6a), called by a hosted session's own agent: a
//! fake `claude` that, once the flow says how, runs the `oso mcp` its `--mcp-config` names as
//! its child, with a FIFO for stdin and a file for stdout, as Claude Code runs its MCP servers.
//! The bridge is let in unasked as that session's. `session_list` and `session_state` read the
//! hosted sessions, `start_worktree_session` asks the person and starts a session in a new
//! worktree on `y`, `n` refuses it and the same call for a minute after, and `worktree_remove`
//! removes a finished worktree with no question and keeps its branch.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the tools' table,
//! the questions' wording, the loop guard and the refusals have their own tests.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::harness::App;
use super::worktrees::{DEADLINE, Flow, asks, terminal};

/// The fake: where it runs and what it was given, a FIFO `in.PID` for its bridge's stdin, and
/// once the flow wrote `bridge.PID`, that script run as its child, writing to `out.PID`. It
/// exits with the status the flow writes to `go.PID`, or to `go` for every run, and ends once
/// the flow's directory is gone. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf '%s\0' "$(pwd -P)" "$@" > "$dir/tmp.$$"
mkfifo "$dir/in.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
printf 'fake claude\n'
bridged=
while [ ! -f "$dir/go" ] && [ ! -f "$dir/go.$$" ]; do
  [ -d "$dir" ] || exit 0
  if [ -z "$bridged" ] && [ -f "$dir/bridge.$$" ]; then
    sh "$dir/bridge.$$" < "$dir/in.$$" > "$dir/out.$$" 2> "$dir/err.$$" &
    bridged=1
  fi
  sleep 0.1
done
if [ -f "$dir/go.$$" ]; then exit "$(cat "$dir/go.$$")"; fi
exit "$(cat "$dir/go")"
"#;

/// How long the app takes no answer to the agent's question after a key, and a moment more.
const QUIET: Duration = Duration::from_millis(700);

/// How long the channel waits after the person's last key before it changes anything, and a
/// moment more.
const AFTER_TYPING: Duration = Duration::from_millis(1200);

/// One run of the fake: its process id, the directory it ran in, and its arguments.
struct Run {
    pid: String,
    cwd: PathBuf,
    args: Vec<String>,
}

/// Each run of the fake in `flow` so far.
fn runs(flow: &Flow) -> Vec<Run> {
    fs::read_dir(flow.scratch.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let pid = name.strip_prefix("run.")?.to_owned();
            let text = fs::read_to_string(entry.path()).ok()?;
            let mut fields: Vec<String> = text.split('\0').map(str::to_owned).collect();
            // Each field ends with a NUL, so the last split is empty.
            fields.pop();
            let cwd = PathBuf::from(fields.first()?);
            Some(Run {
                pid,
                cwd,
                args: fields.split_off(1),
            })
        })
        .collect()
}

/// The fake's run in `cwd`, once it is there.
fn run_in(flow: &Flow, cwd: &Path) -> Run {
    let cwd = fs::canonicalize(cwd).unwrap();
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(run) = runs(flow).into_iter().find(|run| run.cwd == cwd) {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "no fake ran in {}",
            cwd.display()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// A single-quoted word for `sh`.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// The `oso mcp` of one run of the fake, spoken to over its FIFO.
struct Bridge {
    input: File,
    output: PathBuf,
    next: u64,
}

impl Bridge {
    /// Runs the server that `run`'s `--mcp-config` names, as the fake's child, and opens its
    /// stdin. Asks for `initialize` first, as Claude Code does.
    fn start(flow: &Flow, run: &Run) -> Bridge {
        let config = run
            .args
            .iter()
            .find_map(|arg| arg.strip_prefix("--mcp-config="))
            .unwrap_or_else(|| panic!("no --mcp-config in {:?}", run.args));
        let config: Value = serde_json::from_str(config).expect("--mcp-config is JSON");
        let server = &config["mcpServers"]["oso"];
        let mut script = String::new();
        for (name, value) in server["env"].as_object().into_iter().flatten() {
            let value = value.as_str().expect("an environment value is text");
            script.push_str(&format!("export {name}={}\n", quoted(value)));
        }
        let command = server["command"].as_str().expect("the server's command");
        let args: Vec<String> = server["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(quoted)
            .collect();
        script.push_str(&format!("exec {} {}\n", quoted(command), args.join(" ")));
        let dir = flow.scratch.path();
        // Written whole before the fake looks for it.
        let ready = dir.join(format!("bridge-tmp.{}", run.pid));
        fs::write(&ready, script).unwrap();
        fs::rename(&ready, dir.join(format!("bridge.{}", run.pid))).unwrap();
        // Opening a FIFO waits for its reader: on a thread, so that a fake that never reads
        // fails the flow instead of hanging it.
        let fifo = dir.join(format!("in.{}", run.pid));
        let (opened, open) = mpsc::channel();
        thread::spawn(move || {
            let _ = opened.send(OpenOptions::new().write(true).open(fifo));
        });
        let input = open
            .recv_timeout(DEADLINE)
            .expect("the bridge never read its stdin")
            .expect("cannot open the bridge's stdin");
        let mut bridge = Bridge {
            input,
            output: dir.join(format!("out.{}", run.pid)),
            next: 1,
        };
        let id = bridge.send(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "e2e", "version": "1" },
            }),
        );
        bridge.reply(id);
        bridge.notify("notifications/initialized");
        bridge
    }

    /// Sends a request, and returns its id.
    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.input, "{request}").expect("cannot write to the bridge");
        id
    }

    /// Sends a notification.
    fn notify(&mut self, method: &str) {
        let note = json!({ "jsonrpc": "2.0", "method": method });
        writeln!(self.input, "{note}").expect("cannot write to the bridge");
    }

    /// Calls the tool `name` with `arguments`, and returns the request's id.
    fn call(&mut self, name: &str, arguments: Value) -> u64 {
        self.send(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    /// The response to request `id`, once it is there.
    fn reply(&self, id: u64) -> Value {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let text = fs::read_to_string(&self.output).unwrap_or_default();
            let found = text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|response| response["id"] == id);
            if let Some(response) = found {
                return response;
            }
            assert!(Instant::now() < deadline, "no reply to {id}: {text}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// The result of the tool call `id`: what it said, parsed if it is JSON, and whether it is
    /// an error.
    fn result(&self, id: u64) -> (Value, bool) {
        let response = self.reply(id);
        let result = &response["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text in {response}"));
        let said = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()));
        (said, result["isError"] == true)
    }

    /// Calls the tool `name` with `arguments` and waits for its result.
    fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        let id = self.call(name, arguments);
        self.result(id)
    }
}

/// Starts an instance in `flow`'s repository and a session at its root, and returns the
/// instance and that session's bridge.
fn hosted(flow: &Flow) -> (App, Bridge) {
    let (mut app, _) = flow.start();
    app.send_keys("C-x a c");
    let run = run_in(flow, &flow.repo);
    app.wait_for("the session's terminal", |state| {
        terminal(state, "*claude: repo*").is_some()
    });
    let bridge = Bridge::start(flow, &run);
    (app, bridge)
}

/// Waits for the agent's question, and answers it with `key` as the person.
fn answer(app: &mut App, key: &str) -> String {
    let state = app.wait_for("the agent's question", |state| {
        asks(state).is_some_and(|prompt| prompt.starts_with("Agent: "))
    });
    thread::sleep(QUIET);
    app.send_keys(key);
    asks(&state).unwrap_or_default()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_sessions_agent_lists_the_sessions_and_reads_one() {
    let flow = Flow::with_fake("session-tools-read", "", FAKE);
    let (_app, mut bridge) = hosted(&flow);

    let (list, error) = bridge.tool("session_list", json!({}));
    assert!(!error, "{list}");
    let sessions = list["sessions"].as_array().expect("a list of sessions");
    assert_eq!(sessions.len(), 1, "{list}");
    let ours = &sessions[0];
    assert_eq!(ours["name"], "repo");
    assert_eq!(ours["you"], true);
    assert_eq!(list["you"], ours["id"]);
    assert!(
        ours["state"].as_str().is_some_and(|s| !s.is_empty()),
        "{ours}"
    );

    let (state, error) = bridge.tool("session_state", json!({ "session": "repo" }));
    assert!(!error, "{state}");
    assert_eq!(state["id"], ours["id"]);
    assert_eq!(state["review"], Value::Null);
    let (said, error) = bridge.tool("session_state", json!({ "session": "nobody" }));
    assert!(error, "{said}");
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_allowed_worktree_session_starts_and_its_finished_worktree_is_removed() {
    let flow = Flow::with_fake("session-tools-start", "", FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let place = flow.repo.join(".claude/worktrees/e2e-agent");

    let id = bridge.call(
        "start_worktree_session",
        json!({ "branch": "e2e-agent", "brief": "Fix the parser" }),
    );
    let question = answer(&mut app, "y");
    assert!(question.contains("e2e-agent"), "{question}");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    assert_eq!(started["session"]["name"], "e2e-agent");
    let run = run_in(&flow, &place);
    app.wait_for("the worktree session's terminal", |state| {
        terminal(state, "*claude: e2e-agent*").is_some()
    });

    // Its session ends, so no busy session keeps the worktree, and its work is merged into
    // main and pushed.
    flow.scratch.write(&format!("go.{}", run.pid), b"0");
    fs::write(place.join("work.txt"), "work\n").unwrap();
    flow.git(&place, &["add", "work.txt"]);
    flow.git(&place, &["commit", "-q", "--no-gpg-sign", "-m", "work"]);
    flow.git(&flow.repo, &["merge", "-q", "--ff-only", "e2e-agent"]);
    flow.git(&flow.repo, &["push", "-q", "origin", "main"]);
    app.wait_for("the worktree session's stop", |state| {
        terminal(state, "*claude: e2e-agent*")
            .is_some_and(|terminal| terminal["ended"] == "stopped: session-resume to continue")
    });
    let path = place.display().to_string();
    // The person's `y` was a key: changes wait until they stop typing.
    thread::sleep(AFTER_TYPING);
    let (removed, error) = bridge.tool("worktree_remove", json!({ "path": path }));
    assert!(!error, "{removed}");
    assert_eq!(removed["branch_kept"], true);
    assert_eq!(removed["deleted_ignored"], json!([".env"]));
    assert!(!place.exists(), "the worktree is still there");
    let listed = flow.git(&flow.repo, &["branch", "--list", "e2e-agent"]);
    assert!(listed.contains("e2e-agent"), "the branch went");
    let state = app.state();
    let echo = state["echo"].as_str().unwrap_or_default();
    assert!(echo.starts_with("Session repo removed worktree "), "{echo}");
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_refused_worktree_session_is_refused_again_without_asking() {
    let flow = Flow::with_fake("session-tools-refuse", "", FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let call = json!({ "branch": "e2e-refused", "brief": "Fix the parser" });

    let id = bridge.call("start_worktree_session", call.clone());
    answer(&mut app, "n");
    let (said, error) = bridge.result(id);
    assert!(error, "{said}");
    assert!(
        said.as_str().is_some_and(|said| said.contains("decision")),
        "{said}"
    );

    // The same call within the minute: refused at once, and nobody is asked.
    let (said, error) = bridge.tool("start_worktree_session", call);
    assert!(error, "{said}");
    assert!(
        said.as_str().is_some_and(|said| said.contains("decision")),
        "{said}"
    );
    let state = app.state();
    assert_eq!(state["prompts"], json!([]));
    assert_eq!(runs(&flow).len(), 1, "a second fake ran");
    assert!(!flow.repo.join(".claude/worktrees/e2e-refused").exists());
    flow.exit_with(0);
}
