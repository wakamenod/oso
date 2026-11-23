//! The agents' session tools (ADR 0121, step 6a), called by a hosted session's own agent: a
//! fake `claude` that, once the flow says how, runs the `oso mcp` its `--mcp-config` names as
//! its child, with a FIFO for stdin and a file for stdout, as Claude Code runs its MCP servers.
//! The bridge is let in unasked as that session's. `session_list` and `session_state` read the
//! hosted sessions, `start_worktree_session` asks the person and starts a session in a new
//! worktree on `y`, `n` refuses it and the same call for a minute after, and `worktree_remove`
//! removes a finished worktree with no question and keeps its branch, also one whose session,
//! idle at its prompt, was carried over a restart and judged idle there. With
//! `[sessions] agents_start_without_asking`, the start asks nothing up to the limit, says so and
//! marks the row, asks past the limit, asks again after `agent-starts-ask`, and asks for a
//! `session_start` once the person no longer trusts the project (ADR 0135). A worktree
//! session whose turn leaves work not merged is ready for review until the person's listing,
//! or an agent's of its space, finds it merged; a turn that ends on merged work leaves it done, and a removal takes a worktree whose
//! session is ready for review once its work is merged (ADR 0121, "Merged work is not ready for
//! review, as amended"). A worktree session whose agent never called oso is known by its
//! process all the same, and its finished worktree is removed.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the tools' table,
//! the questions' wording and the refusals have their own tests.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::harness::App;
use super::worktrees::{DEADLINE, Flow, asks, keys, terminal};

/// The fake: where it runs and what it was given, a FIFO `in.PID` for its bridge's stdin, and
/// once the flow wrote `bridge.PID`, that script run as its child, writing to `out.PID`. It
/// prints each `screen.PID` the flow writes, cleared first, as the TUI would. It
/// exits with the status the flow writes to `go.PID`, or to `go` for every run, and ends once
/// the flow's directory is gone. `DIR` is the flow's scratch directory.
/// Asked to name a space, it does as every fake does (`worktrees::NAMING`).
pub(super) const FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf '%s\0' "$(pwd -P)" "$@" > "$dir/tmp.$$"
mkfifo "$dir/in.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
printf 'fake claude\n'
bridged=
while [ ! -f "$dir/go" ] && [ ! -f "$dir/go.$$" ]; do
  [ -d "$dir" ] || exit 0
  if [ -f "$dir/screen.$$" ]; then
    printf '\033[2J\033[H'
    cat "$dir/screen.$$"
    rm -f "$dir/screen.$$"
  fi
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
pub(super) struct Run {
    pub(super) pid: String,
    pub(super) cwd: PathBuf,
    pub(super) args: Vec<String>,
}

/// Each run of the fake in `flow` so far.
pub(super) fn runs(flow: &Flow) -> Vec<Run> {
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
pub(super) fn run_in(flow: &Flow, cwd: &Path) -> Run {
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
pub(super) struct Bridge {
    input: File,
    output: PathBuf,
    /// Where the server's standard error goes.
    errors: PathBuf,
    next: u64,
}

impl Bridge {
    /// Runs the server that `run`'s `--mcp-config` names, as the fake's child, and opens its
    /// stdin. Asks for `initialize` first, as Claude Code does.
    pub(super) fn start(flow: &Flow, run: &Run) -> Bridge {
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
            errors: dir.join(format!("err.{}", run.pid)),
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
    pub(super) fn call(&mut self, name: &str, arguments: Value) -> u64 {
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

    /// What the server wrote so far, to standard output and standard error, for a flow that
    /// fails waiting on it.
    fn said(&self) -> String {
        let read = |path: &Path| fs::read_to_string(path).unwrap_or_default();
        format!(
            "its stdout:\n{}\nits stderr:\n{}",
            read(&self.output),
            read(&self.errors)
        )
    }

    /// The result of the tool call `id`: what it said, parsed if it is JSON, and whether it is
    /// an error.
    pub(super) fn result(&self, id: u64) -> (Value, bool) {
        let response = self.reply(id);
        let result = &response["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text in {response}"));
        let said = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()));
        (said, result["isError"] == true)
    }

    /// Calls the tool `name` with `arguments` and waits for its result.
    pub(super) fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        let id = self.call(name, arguments);
        self.result(id)
    }

    /// As [`Bridge::tool`], called again, as an agent does, while the channel says the person
    /// is busy, typing or so (ADR 0085), for at most [`DEADLINE`]. A slow machine can keep a
    /// call within that wait however long the flow sleeps first.
    pub(super) fn tool_when_free(&mut self, name: &str, arguments: &Value) -> (Value, bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let (said, error) = self.tool(name, arguments.clone());
            if !(error && said.to_string().contains("try again in a moment"))
                || Instant::now() > deadline
            {
                return (said, error);
            }
            thread::sleep(Duration::from_millis(300));
        }
    }
}

/// Starts an instance in `flow`'s repository and a session at its root, and returns the
/// instance and that session's bridge.
fn hosted(flow: &Flow) -> (App, Bridge) {
    let (app, bridge, _) = hosted_with_readme(flow);
    (app, bridge)
}

/// As [`hosted`], with the id of the `README.md` buffer the instance opened.
fn hosted_with_readme(flow: &Flow) -> (App, Bridge, Value) {
    let (mut app, readme) = flow.start();
    app.send_keys("C-c a c");
    let run = run_in(flow, &flow.repo);
    app.wait_for("the session's terminal", |state| {
        terminal(state, "*claude: repo*").is_some()
    });
    let bridge = Bridge::start(flow, &run);
    (app, bridge, readme)
}

/// Waits for the question an agent's call on `bridge` asks, and answers it with `key` as the
/// person. One that does not come fails with what was asked instead, the echo area and what
/// the bridge said, which tell a call that never reached the app from another question.
pub(super) fn answer(app: &mut App, bridge: &Bridge, key: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    let state = loop {
        let state = app.state();
        if asks(&state).is_some_and(|prompt| prompt.starts_with("Agent: ")) {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "no agent's question after {DEADLINE:?}; asked instead: {}; the echo area: {}; the bridge: {}",
            state["prompts"],
            state["echo"],
            bridge.said(),
        );
        thread::sleep(Duration::from_millis(100));
    };
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
    let before = selected(&mut app);
    assert_eq!(before.1, "*claude: repo*");

    let id = bridge.call(
        "start_worktree_session",
        json!({ "branch": "e2e-agent", "brief": "Fix the parser in @src/lib.rs" }),
    );
    let question = answer(&mut app, &bridge, "y");
    assert!(question.contains("e2e-agent"), "{question}");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    assert_eq!(started["session"]["name"], "e2e-agent");
    let run = run_in(&flow, &place);
    // An agent's brief is escaped, as text oso was given (ADR 0121, "An agent's brief, as
    // amended").
    assert!(
        run.args
            .iter()
            .any(|arg| arg.ends_with("\n\nFix the parser in \\@src/lib.rs")),
        "{:?}",
        run.args
    );
    app.wait_for("the worktree session's terminal", |state| {
        terminal(state, "*claude: e2e-agent*").is_some()
    });
    assert_eq!(
        selected(&mut app),
        before,
        "the start stayed in the background"
    );

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
    // Its stopped session is archived with it (ADR 0121, "Removal archives its sessions").
    assert_eq!(removed["archived"], 1, "{removed}");
    assert!(!place.exists(), "the worktree is still there");
    let listed = flow.git(&flow.repo, &["branch", "--list", "e2e-agent"]);
    assert!(listed.contains("e2e-agent"), "the branch went");
    let state = app.state();
    let echo = state["echo"].as_str().unwrap_or_default();
    assert!(echo.starts_with("Session repo removed worktree "), "{echo}");
    assert!(echo.ends_with("; 1 session in it archived"), "{echo}");
    let archived = super::worktrees::session_archived(&state, "e2e-agent");
    assert_eq!(archived, Some(true), "{state}");
    flow.exit_with(0);
}

/// A worktree session idle at its prompt, carried over a restart, is judged idle in the new
/// instance though its screen never changes, and so its finished worktree is removed: unjudged,
/// it stayed busy, and every removal was refused.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_idle_session_carried_over_a_restart_is_judged_and_its_worktree_removed() {
    let flow = Flow::with_fake("session-tools-carried-idle", "", FAKE);
    let (mut old, mut bridge) = hosted(&flow);
    let place = flow.repo.join(".claude/worktrees/e2e-idle");
    let id = bridge.call(
        "start_worktree_session",
        json!({ "branch": "e2e-idle", "brief": "Fix the parser" }),
    );
    answer(&mut old, &bridge, "y");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    let run = run_in(&flow, &place);
    old.wait_for("the worktree session's terminal", |state| {
        terminal(state, "*claude: e2e-idle*").is_some()
    });
    registered_idle(&flow, &run, &place);
    // Its work is merged into main and pushed; the session stays at its prompt.
    fs::write(place.join("work.txt"), "work\n").unwrap();
    flow.git(&place, &["add", "work.txt"]);
    flow.git(&place, &["commit", "-q", "--no-gpg-sign", "-m", "work"]);
    flow.git(&flow.repo, &["merge", "-q", "--ff-only", "e2e-idle"]);
    flow.git(&flow.repo, &["push", "-q", "origin", "main"]);

    let mut new = flow.start_for_restart();
    crate::terminal::restart::hand_over(&mut old, &mut new);
    new.wait_for("the carried session", |state| {
        terminal(state, "*claude: e2e-idle*").is_some_and(|terminal| terminal["running"] == true)
    });
    // The bridge connects again, to the new instance, on its next call.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (state, error) = bridge.tool("session_state", json!({ "session": "e2e-idle" }));
        if !error && state["state"] == "idle" {
            break;
        }
        assert!(Instant::now() < deadline, "not judged idle: {state}");
        thread::sleep(Duration::from_millis(200));
    }
    let path = place.display().to_string();
    let (removed, error) = bridge.tool("worktree_remove", json!({ "path": path }));
    assert!(!error, "{removed}");
    assert!(!place.exists(), "the worktree is still there");
    flow.exit_with(0);
}

const WORKING: &str = include_str!("../../src/sessions/screens/working.txt");
const DONE: &str = include_str!("../../src/sessions/screens/done.txt");

/// Has `run` of the fake print `screen`.
fn print(flow: &Flow, run: &Run, screen: &str) {
    let staged = flow
        .scratch
        .write(&format!("screen.{}.new", run.pid), screen.as_bytes());
    fs::rename(
        staged,
        flow.scratch.path().join(format!("screen.{}", run.pid)),
    )
    .unwrap();
}

/// Session `name`'s state in the dev state.
fn state_of(state: &Value, name: &str) -> Option<String> {
    state["sessions"]["sessions"]
        .as_array()?
        .iter()
        .find(|session| session["name"] == name)?["state"]
        .as_str()
        .map(str::to_owned)
}

/// Has session `name`, run `run` of the fake, take a turn that ends: working, then done or,
/// with changes not merged, ready for review. Returns the first state seen after working:
/// done can be passed unseen on the way to ready for review.
fn turn(app: &mut App, flow: &Flow, run: &Run, name: &str) -> String {
    print(flow, run, WORKING);
    app.wait_for("working", |state| {
        state_of(state, name).as_deref() == Some("working")
    });
    print(flow, run, DONE);
    let state = app.wait_for("the turn's end", |state| {
        state_of(state, name).is_some_and(|s| s != "working")
    });
    state_of(&state, name).unwrap()
}

/// Starts a worktree session on `branch` from the session at the repository's root, idle in
/// Claude Code's registry, and commits work there. Returns its place and its run.
fn worktree_with_work(
    app: &mut App,
    bridge: &mut Bridge,
    flow: &Flow,
    branch: &str,
) -> (PathBuf, Run) {
    let place = flow.repo.join(".claude/worktrees").join(branch);
    let id = bridge.call(
        "start_worktree_session",
        json!({ "branch": branch, "brief": "Fix the parser" }),
    );
    answer(app, bridge, "y");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    let run = run_in(flow, &place);
    app.wait_for("the worktree session's terminal", |state| {
        terminal(state, &format!("*claude: {branch}*")).is_some()
    });
    registered_idle(flow, &run, &place);
    // Its own bridge connects, as a real session's does at its agent's first oso call. oso
    // knows its process from its start either way, which the registry's half of check 4 needs.
    let mut own = Bridge::start(flow, &run);
    let (_, error) = own.tool("session_list", json!({}));
    assert!(!error);
    fs::write(place.join("work.txt"), "work\n").unwrap();
    flow.git(&place, &["add", "work.txt"]);
    flow.git(&place, &["commit", "-q", "--no-gpg-sign", "-m", "work"]);
    (place, run)
}

/// Merges `branch` into main and pushes it, as a merged pull request would.
fn merge(flow: &Flow, branch: &str) {
    flow.git(&flow.repo, &["merge", "-q", "--ff-only", branch]);
    flow.git(&flow.repo, &["push", "-q", "origin", "main"]);
}

/// Work not merged is ready for review; once merged, the person's listing makes it idle, a
/// turn that ends on it leaves it done, new work makes it ready for review again until an agent
/// of its space lists it merged, and its worktree is removed.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn merged_work_is_not_ready_for_review() {
    let flow = Flow::with_fake("session-tools-merged-work", "", FAKE);
    let (mut app, mut bridge, readme) = hosted_with_readme(&flow);
    let (place, run) = worktree_with_work(&mut app, &mut bridge, &flow, "e2e-merged");
    let name = "e2e-merged";
    // Done lasts only while the look and the merged check run, which a fast machine can pass
    // between two reads of the state.
    let ended = turn(&mut app, &flow, &run, name);
    assert!(
        matches!(ended.as_str(), "done" | "ready for review"),
        "{ended}"
    );
    app.wait_for("ready for review", |state| {
        state_of(state, name).as_deref() == Some("ready for review")
    });

    merge(&flow, name);
    // The person's listing reaches every session.
    app.show_buffer(&readme);
    app.send_keys(&format!("M-x {} RET", keys("worktree-list")));
    app.wait_for("*Worktrees*", |state| {
        state["buffer"]["name"] == "*Worktrees*"
    });
    app.wait_for("idle once the person's listing found it merged", |state| {
        state_of(state, name).as_deref() == Some("idle")
    });
    // The next turn's end on merged work leaves it done; an agent of its space then lists.

    assert_eq!(turn(&mut app, &flow, &run, name), "done");
    // The look and the merged check that follow it take well under a second.
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        state_of(&app.state(), name).as_deref(),
        Some("done"),
        "merged work after a turn is not ready for review"
    );
    // A new commit, not merged: ready for review again, until an agent of its space lists it
    // merged.
    fs::write(place.join("more.txt"), "more\n").unwrap();
    flow.git(&place, &["add", "more.txt"]);
    flow.git(&place, &["commit", "-q", "--no-gpg-sign", "-m", "more"]);
    turn(&mut app, &flow, &run, name);
    app.wait_for("ready for review again", |state| {
        state_of(state, name).as_deref() == Some("ready for review")
    });
    merge(&flow, name);
    let (listed, error) = bridge.tool("worktree_list", json!({}));
    assert!(!error, "{listed}");
    app.wait_for("idle once its space's agent found it merged", |state| {
        state_of(state, name).as_deref() == Some("idle")
    });

    thread::sleep(AFTER_TYPING);
    let path = place.display().to_string();
    let (removed, error) = bridge.tool("worktree_remove", json!({ "path": path }));
    assert!(!error, "{removed}");
    assert!(!place.exists(), "the worktree is still there");
    flow.exit_with(0);
}

/// A removal does not wait for a listing: a session ready for review whose work has been
/// merged since does not keep its worktree, and is stopped before git removes it.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_removal_takes_a_worktree_whose_reviewable_work_was_merged() {
    let flow = Flow::with_fake("session-tools-merged-remove", "", FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let (place, run) = worktree_with_work(&mut app, &mut bridge, &flow, "e2e-reviewable");
    let name = "e2e-reviewable";
    turn(&mut app, &flow, &run, name);
    app.wait_for("ready for review", |state| {
        state_of(state, name).as_deref() == Some("ready for review")
    });
    thread::sleep(AFTER_TYPING);
    let path = place.display().to_string();
    let (kept, error) = bridge.tool("worktree_remove", json!({ "path": path }));
    assert!(error, "{kept}");
    assert!(place.exists(), "work not merged was removed");

    merge(&flow, name);
    let (removed, error) = bridge.tool("worktree_remove", json!({ "path": path }));
    assert!(!error, "{removed}");
    assert!(!place.exists(), "the worktree is still there");
    app.wait_for("its session stopped, as an idle one is", |state| {
        terminal(state, &format!("*claude: {name}*"))
            .is_some_and(|terminal| terminal["ended"] == "stopped: session-resume to continue")
    });
    flow.exit_with(0);
}

/// A hosted session whose registry entry says idle but not when its process started is busy:
/// its worktree is kept, though its screen is settled and its work merged, until its own entry
/// is back.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_idle_registry_entry_with_no_start_time_keeps_its_worktree() {
    let flow = Flow::with_fake("session-tools-unknown-idle", "", FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let (place, run) = worktree_with_work(&mut app, &mut bridge, &flow, "e2e-unknown");
    let name = "e2e-unknown";
    turn(&mut app, &flow, &run, name);
    app.wait_for("ready for review", |state| {
        state_of(state, name).as_deref() == Some("ready for review")
    });
    merge(&flow, name);

    let file = flow
        .home
        .join(".claude/sessions")
        .join(format!("{}.json", run.pid));
    let own = fs::read(&file).unwrap();
    let mut entry: Value = serde_json::from_slice(&own).unwrap();
    let fields = entry.as_object_mut().unwrap();
    fields.remove("startedAt");
    fields.remove("procStart");
    fs::write(&file, entry.to_string()).unwrap();
    thread::sleep(AFTER_TYPING);
    let path = place.display().to_string();
    let (kept, error) = bridge.tool("worktree_remove", json!({ "path": path }));
    assert!(error, "{kept}");
    assert!(
        kept.as_str().is_some_and(|said| said.contains("busy")),
        "{kept}"
    );
    assert!(
        place.exists(),
        "an unverifiable entry let the removal through"
    );

    fs::write(&file, own).unwrap();
    let (removed, error) = bridge.tool("worktree_remove", json!({ "path": path }));
    assert!(!error, "{removed}");
    assert!(!place.exists(), "the worktree is still there");
    flow.exit_with(0);
}

/// A worktree session whose agent never called oso, so that no bridge of its own ever claimed
/// it, is known by its process from its start. Done after its turn and idle in Claude Code's
/// registry, by an entry that carries no id oso gave, it is not listed as another's session,
/// and its finished worktree is removed with the session stopped. oso read the process id
/// before the terminal's thread had spawned it, and every such removal was refused as busy.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_idle_session_with_no_bridge_of_its_own_is_known_and_its_worktree_removed() {
    let flow = Flow::with_fake("session-tools-unbridged-idle", "", FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let name = "e2e-unbridged";
    let place = flow.repo.join(".claude/worktrees").join(name);
    let id = bridge.call(
        "start_worktree_session",
        json!({ "branch": name, "brief": "Fix the parser" }),
    );
    answer(&mut app, &bridge, "y");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    let run = run_in(&flow, &place);
    app.wait_for("the worktree session's terminal", |state| {
        terminal(state, &format!("*claude: {name}*")).is_some()
    });
    registered_idle(&flow, &run, &place);
    fs::write(place.join("work.txt"), "work\n").unwrap();
    flow.git(&place, &["add", "work.txt"]);
    flow.git(&place, &["commit", "-q", "--no-gpg-sign", "-m", "work"]);
    merge(&flow, name);
    // A turn that ends on merged work leaves it done, settled as a session at its prompt is.
    let ended = turn(&mut app, &flow, &run, name);
    assert!(
        matches!(ended.as_str(), "done" | "ready for review"),
        "{ended}"
    );

    // The listing finds it merged and removable, and names no session of another's in it.
    let (listed, error) = bridge.tool_when_free("worktree_list", &json!({}));
    assert!(!error, "{listed}");
    let row = (listed["worktrees"].as_array().into_iter().flatten())
        .find(|row| row["branch"] == name)
        .unwrap_or_else(|| panic!("not listed: {listed}"));
    assert_eq!(row["other_sessions"], json!([]), "{row}");
    assert_eq!(row["merged"], true, "{row}");
    assert_eq!(row["removable"], true, "{row}");

    thread::sleep(AFTER_TYPING);
    let path = place.display().to_string();
    let (removed, error) = bridge.tool_when_free("worktree_remove", &json!({ "path": path }));
    assert!(!error, "{removed}");
    assert!(!place.exists(), "the worktree is still there");
    app.wait_for("its session stopped, as an idle one is", |state| {
        terminal(state, &format!("*claude: {name}*"))
            .is_some_and(|terminal| terminal["ended"] == "stopped: session-resume to continue")
    });
    flow.exit_with(0);
}

/// Has Claude Code's registry, under the flow's home, say `run` is idle in `cwd`, as a
/// `claude` at its prompt does: the other half of a removal's check that it is not busy. Its
/// `startedAt` is the process's own start, which check 4 matches, as the CLI's is.
fn registered_idle(flow: &Flow, run: &Run, cwd: &Path) {
    let millis = |time: std::time::SystemTime| {
        time.duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    };
    let now = millis(std::time::SystemTime::now());
    let pid = run.pid.parse::<u32>().unwrap();
    let started = oso_platform::process::start_time(pid)
        .expect("the system says when the fake started")
        .expect("the fake runs");
    let registry = flow.home.join(".claude").join("sessions");
    fs::create_dir_all(&registry).unwrap();
    let entry = json!({
        "pid": pid,
        "cwd": cwd,
        "startedAt": millis(started),
        "kind": "interactive",
        "status": "idle",
        "statusUpdatedAt": now,
    });
    fs::write(
        registry.join(format!("{}.json", run.pid)),
        entry.to_string(),
    )
    .unwrap();
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_refused_worktree_session_is_refused_again_without_asking() {
    let flow = Flow::with_fake("session-tools-refuse", "", FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let call = json!({ "branch": "e2e-refused", "brief": "Fix the parser" });

    let id = bridge.call("start_worktree_session", call.clone());
    answer(&mut app, &bridge, "n");
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

/// The selected window and the name of the buffer it shows: what an agent's start leaves as it
/// was (ADR 0144).
fn selected(app: &mut App) -> (Value, Value) {
    let state = app.state();
    let window = state["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|window| window["selected"] == true)
        .map(|window| window["id"].clone())
        .unwrap_or_default();
    (window, state["buffer"]["name"].clone())
}

/// The rows of the buffer the instance shows, read after `keys` select it.
fn rows_after(app: &mut App, keys: &str, name: &str) -> String {
    app.send_keys(keys);
    app.wait_for(name, |state| state["buffer"]["name"] == name);
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 200 })));
    rows["rows"].to_string()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_worktree_session_starts_without_asking_up_to_the_limit_and_asks_past_it() {
    let sessions = "agents_start_without_asking = true\nagents_start_limit = 1\n";
    let flow = Flow::with_fake("session-tools-unasked", sessions, FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let place = flow.repo.join(".claude/worktrees/e2e-unasked");
    let before = selected(&mut app);

    let (started, error) = bridge.tool(
        "start_worktree_session",
        json!({ "branch": "e2e-unasked", "brief": "Fix the parser" }),
    );
    assert!(!error, "{started}");
    assert_eq!(started["session"]["name"], "e2e-unasked");
    assert_eq!(started["asked"], false, "{started}");
    assert_eq!(app.state()["prompts"], json!([]), "nobody was asked");
    run_in(&flow, &place);
    app.wait_for("the unasked session's terminal", |state| {
        terminal(state, "*claude: e2e-unasked*").is_some()
    });
    assert_eq!(
        selected(&mut app),
        before,
        "the start stayed in the background"
    );
    let messages = rows_after(&mut app, "C-x b * M e s s a g e s * RET", "*Messages*");
    assert!(
        messages.contains("Agent: session \\\"repo\\\" is starting a session in a new worktree"),
        "{messages}"
    );
    assert!(messages.contains("without asking"), "{messages}");
    let list = rows_after(&mut app, "C-c a s", "*Spaces*");
    assert!(list.contains("unasked"), "{list}");

    // The limit is 1 and that session runs: the next start asks, and `n` refuses it.
    let id = bridge.call(
        "start_worktree_session",
        json!({ "branch": "e2e-asked", "brief": "Fix the lexer" }),
    );
    let question = answer(&mut app, &bridge, "n");
    assert!(question.contains("e2e-asked"), "{question}");
    let (said, error) = bridge.result(id);
    assert!(error, "{said}");
    assert!(!flow.repo.join(".claude/worktrees/e2e-asked").exists());
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn with_no_limit_worktree_sessions_start_without_asking() {
    let sessions = "agents_start_without_asking = true\nagents_start_limit = \"none\"\n";
    let flow = Flow::with_fake("session-tools-unlimited", sessions, FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let before = selected(&mut app);
    // Past the default limit of 4.
    for n in 0..5 {
        let branch = format!("e2e-many-{n}");
        let (started, error) = bridge.tool(
            "start_worktree_session",
            json!({ "branch": branch, "brief": "Fix the parser" }),
        );
        assert!(!error, "{started}");
        assert_eq!(started["asked"], false, "{started}");
        run_in(&flow, &flow.repo.join(".claude/worktrees").join(&branch));
        let name = format!("*claude: {branch}*");
        app.wait_for("the unasked session's terminal", |state| {
            terminal(state, &name).is_some()
        });
    }
    assert_eq!(app.state()["prompts"], json!([]), "nobody was asked");
    assert_eq!(
        selected(&mut app),
        before,
        "the starts stayed in the background"
    );
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn after_agent_starts_ask_a_start_asks_again() {
    let sessions = "agents_start_without_asking = true\n";
    let flow = Flow::with_fake("session-tools-held", sessions, FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    app.run_command("agent-starts-ask", None).unwrap();
    app.wait_for("the commands' line", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.starts_with("Sessions started by agents ask you first"))
    });
    let id = bridge.call(
        "start_worktree_session",
        json!({ "branch": "e2e-held", "brief": "Fix the parser" }),
    );
    answer(&mut app, &bridge, "n");
    let (said, error) = bridge.result(id);
    assert!(error, "{said}");
    assert_eq!(runs(&flow).len(), 1, "a second fake ran");
    flow.exit_with(0);
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_in_its_own_directory_starts_without_asking_until_the_project_is_untrusted() {
    let sessions = "agents_start_without_asking = true\n";
    let flow = Flow::with_fake("session-tools-own", sessions, FAKE);
    let (mut app, mut bridge) = hosted(&flow);
    let before = selected(&mut app);

    let (started, error) = bridge.tool("session_start", json!({ "brief": "Look around" }));
    assert!(!error, "{started}");
    assert_eq!(started["asked"], false, "{started}");
    assert_eq!(app.state()["prompts"], json!([]), "nobody was asked");
    let name = started["session"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    app.wait_for("the unasked session's terminal", |state| {
        terminal(state, &format!("*claude: {name}*")).is_some()
    });
    assert_eq!(
        selected(&mut app),
        before,
        "the start stayed in the background"
    );

    // Once the person untrusts the project, which both sessions' buffers are in, the same call
    // asks.
    app.run_command("untrust-project", None).unwrap();
    app.wait_for("the project restricted", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.ends_with("is restricted again"))
    });
    thread::sleep(AFTER_TYPING);
    let id = bridge.call("session_start", json!({ "brief": "Look again" }));
    answer(&mut app, &bridge, "n");
    let (said, error) = bridge.result(id);
    assert!(error, "{said}");
    flow.exit_with(0);
}
