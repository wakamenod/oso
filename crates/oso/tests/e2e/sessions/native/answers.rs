//! A space's orchestrator answers its native worker's tool call (ADR 0186), with one fake
//! `claude` and no real one: the space's orchestrator runs natively in the space's directory,
//! and the flow speaks to the `oso mcp` its `--mcp-config` names, as `session_tools` does; the
//! worker it starts runs natively in a worktree, and its fake asks the person's permission
//! through `oso mcp --permission`, as `claude` calls its `--permission-prompt-tool`. The
//! orchestrator is listed the worker's `Edit` and allows it: the worker's bridge gets the one
//! allow, and the call's row says who allowed it. The person answers the next one first, and
//! the orchestrator's answer to it is refused, saying so.
//!
//! Must not depend on: the app's internals, or what the rules decide, which have their own
//! tests. Driven by the control socket and the fakes alone; no real key event is posted.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::super::session_tools::{Bridge, answer as agent_answer, run_in};
use super::super::super::spaces::{PrivateRoot, answer as reply_with, space};
use super::super::super::worktrees::{DEADLINE, Flow, keys};
use super::request::{Keyboard, answer, go, keyboard, until};
use super::text;

/// The fake. Asked to name a space, it does as every fake does (`worktrees::NAMING`). It logs
/// where it runs and its arguments to `run.PID`, as `session_tools::FAKE` does, and one argument
/// a line to `args.PID`. In a worktree it is the worker: once the flow writes `edit.path`, it
/// prints the call `t1`, an `Edit` of that file, and asks its bridge about it; once answered, it
/// prints `t1`'s result and the call `t2`, and asks again; the answers go to `answered`. Else it
/// is the orchestrator: it runs the script the flow writes to `bridge.PID` as its child, its
/// stdin the FIFO `in.PID`, as `session_tools::FAKE` does. Each waits for `go` to exit.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
printf '%s\0' "$(pwd -P)" "$@" > "$dir/tmp.$$"
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/args.$$"
mkfifo "$dir/in.$$"
mv "$dir/tmp.$$" "$dir/run.$$"
case "$(pwd -P)" in
*/.claude/worktrees/*)
  while IFS= read -r first; do
    case "$first" in *'"control_request"'*) ;; *) break ;; esac
  done
  while [ ! -f "$dir/edit.path" ]; do sleep 0.1; done
  file=$(cat "$dir/edit.path")
  call() {
    printf '{"type":"assistant","message":{"id":"m%s","content":[{"type":"tool_use","id":"%s","name":"Edit","input":{"file_path":"%s","old_string":"%s","new_string":"%s"}}]}}\n' "$1" "$1" "$file" "$2" "$3"
  }
  ask() {
    printf '{"jsonrpc":"2.0","id":%s,"method":"tools/call","params":{"name":"permission","arguments":{"tool_name":"Edit","input":{"file_path":"%s","old_string":"%s","new_string":"%s"},"tool_use_id":"%s"}}}\n' "$1" "$file" "$3" "$4" "$2" >&3
  }
  lines() {
    while [ "$(wc -l < "$dir/answered.tmp" | tr -d ' ')" -lt "$1" ]; do sleep 0.1; done
  }
  oso=$(sed -n 's/.*"command":"\([^"]*\)".*/\1/p' "$dir/args.$$" | head -n 1)
  pid=$(sed -n 's/.*"--pid","\([0-9]*\)".*/\1/p' "$dir/args.$$" | head -n 1)
  printf '{"type":"system","subtype":"init","cwd":"%s","session_id":"s","model":"m"}\n' "$(pwd -P)"
  call t1 e2e E2E
  mkfifo "$dir/ask.fifo"
  : > "$dir/answered.tmp"
  "$oso" mcp --pid "$pid" --permission < "$dir/ask.fifo" > "$dir/answered.tmp" &
  exec 3> "$dir/ask.fifo"
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' >&3
  ask 2 t1 e2e E2E
  lines 2
  printf '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"The file was updated."}]}}\n'
  call t2 E2E e2e
  ask 3 t2 E2E e2e
  lines 3
  cp "$dir/answered.tmp" "$dir/answered"
  ;;
*)
  bridged=
  while [ ! -f "$dir/go" ]; do
    [ -d "$dir" ] || exit 0
    if [ -z "$bridged" ] && [ -f "$dir/bridge.$$" ]; then
      sh "$dir/bridge.$$" < "$dir/in.$$" > "$dir/out.$$" 2> "$dir/err.$$" &
      bridged=1
    fi
    sleep 0.1
  done
  ;;
esac
while [ ! -f "$dir/go" ]; do [ -d "$dir" ] || exit 0; sleep 0.1; done
exit 0
"#;

/// The worker's name, which its request's line is named for, as `request`'s helpers have it.
const WORKER: &str = "proj";

/// What the fake's bridge was answered, the `initialize` first, once both calls were.
fn answered(flow: &Flow) -> Vec<Value> {
    let path = flow.scratch.path().join("answered");
    let deadline = Instant::now() + DEADLINE;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "the worker's bridge got no answers"
        );
        thread::sleep(Duration::from_millis(100));
    }
    (fs::read_to_string(&path).unwrap().lines())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The answer the worker's bridge gave its CLI for the call it asked with id `id`, once it did.
fn answer_to(flow: &Flow, id: u64) -> Value {
    let path = flow.scratch.path().join("answered.tmp");
    let deadline = Instant::now() + DEADLINE;
    loop {
        let found = (fs::read_to_string(&path).unwrap_or_default().lines())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|line| line["id"] == id);
        if let Some(line) = found {
            let said = line["result"]["content"][0]["text"].as_str().unwrap();
            return serde_json::from_str(said).unwrap();
        }
        assert!(Instant::now() < deadline, "no answer to {id}");
        thread::sleep(Duration::from_millis(100));
    }
}

/// The worker's `request`, as `session_state` lists it to the orchestrator, once one with an id
/// other than `after` is.
fn listed(bridge: &mut Bridge, after: Option<&str>) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (state, error) = bridge.tool("session_state", json!({ "session": WORKER }));
        assert!(!error, "{state}");
        let request = &state["request"];
        if request.is_object() && request["id"].as_str() != after {
            return request.clone();
        }
        assert!(Instant::now() < deadline, "no request listed: {state}");
        thread::sleep(Duration::from_millis(200));
    }
}

/// Selects buffer `name`.
fn select(app: &mut crate::harness::App, name: &str) {
    app.send_keys("C-x b");
    app.send_keys(&keys(name));
    app.send_keys("RET");
    app.wait_for(name, |state| state["buffer"]["name"] == name);
}

/// The space `fix-login` of the flow's repository, its orchestrator running natively in its
/// directory: the instance and the orchestrator's bridge.
fn orchestrated(flow: &Flow, spaces: &PrivateRoot) -> (crate::harness::App, Bridge) {
    let (mut app, _) = flow.start();
    app.send_keys("C-c a O");
    reply_with(&mut app, "Describe the space:", "Fix login");
    app.wait_for("the space's directory", |state| {
        space(state, "fix-login").is_some_and(|s| s["directory"].is_string())
    });
    let run = run_in(flow, &spaces.0.join("fix-login"));
    assert_eq!(run.args.first().map(String::as_str), Some("-p"), "native");
    let bridge = Bridge::start(flow, &run);
    (app, bridge)
}

/// The worktree the worker runs in, once there is one.
fn worktree(repo: &Path) -> PathBuf {
    let worktrees = repo.join(".claude/worktrees");
    fs::read_dir(&worktrees)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .next()
        .expect("the worker's worktree")
}

/// The worker's next call, listed after `first`, which the person answers first with `n` on its
/// line: the orchestrator's answer to it is refused, saying so.
fn the_person_answers_first(
    flow: &Flow,
    app: &mut crate::harness::App,
    bridge: &mut Bridge,
    first: &str,
) {
    // The next call the person answers first, with `n` on its line.
    let next = listed(bridge, Some(first));
    let second = next["id"].as_str().unwrap().to_owned();
    assert_ne!(second, first);
    until(app, "the request's line", |state| {
        !state["request"].is_null()
    });
    go(app);
    answer(app, "n");
    let denied = answer_to(flow, 3);
    assert_eq!(denied["behavior"], "deny", "{denied}");
    let late = json!({ "session": WORKER, "request": second, "tool": "Edit", "answer": "deny" });
    let (said, error) = bridge.tool_when_free("session_answer", &late);
    assert!(
        error && said.to_string().contains("the person answered it first"),
        "{said}"
    );
    assert_eq!(answered(flow).len(), 3, "the initialize and two answers");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_orchestrator_allows_its_workers_edit_and_the_person_answers_first() {
    let spaces = PrivateRoot::new("native-answers");
    let config = format!(
        "\n[spaces]\nroot = \"{}\"\norchestrator-answers = [\"Edit\"]\n",
        spaces.0.display()
    );
    let flow = Flow::with_fake("native-answers", &config, FAKE);
    // Native sessions, the orchestrator's and the worker's, not the TUI the flows default to.
    let file = flow.scratch.path().join("config.toml");
    let written = fs::read_to_string(&file).unwrap();
    fs::write(&file, written.replace("mode = \"tui\"\n", "")).unwrap();
    let (mut app, mut bridge) = orchestrated(&flow, &spaces);

    // The orchestrator starts the worker, on the person's `y`.
    let call = json!({
        "branch": "fix/login", "brief": "Fix the login error.", "repo": "repo", "name": WORKER,
    });
    let id = bridge.call("start_worktree_session", call);
    agent_answer(&mut app, &bridge, "y");
    let (started, error) = bridge.result(id);
    assert!(!error, "{started}");
    let tree = worktree(&flow.repo);
    let worker = run_in(&flow, &tree);
    assert_eq!(
        worker.args.first().map(String::as_str),
        Some("-p"),
        "native"
    );

    // Its call edits a file of its worktree, as oso has the worker's directory.
    let (state, _) = bridge.tool("session_state", json!({ "session": WORKER }));
    let directory = state["directory"].as_str().expect("the worker's directory");
    let edited = Path::new(directory).join("README.md");
    flow.scratch
        .write("edit.path", edited.display().to_string().as_bytes());
    keyboard(&mut app, Keyboard::Japanese);

    // Listed to the orchestrator, which may allow it, and allows it.
    let request = listed(&mut bridge, None);
    assert_eq!(request["tool"], "Edit", "{request}");
    assert_eq!(request["answerable"], true, "{request}");
    assert_eq!(request["input"]["file_path"], edited.display().to_string());
    let first = request["id"].as_str().unwrap().to_owned();
    // Named with another tool, it is refused, and nothing is answered.
    let wrong = json!({ "session": WORKER, "request": first, "tool": "Bash", "answer": "allow" });
    let (said, error) = bridge.tool("session_answer", wrong);
    assert!(
        error && said.to_string().contains("not the tool you named"),
        "{said}"
    );
    let allow = json!({ "session": WORKER, "request": first, "tool": "Edit", "answer": "allow" });
    // As any change, it waits while the person types, as just now their `y`.
    let (said, error) = bridge.tool_when_free("session_answer", &allow);
    assert!(!error, "{said}");
    assert_eq!(said["answer"], "allowed", "{said}");
    assert!(
        said["summary"]
            .as_str()
            .is_some_and(|s| s.contains("README.md")),
        "{said}"
    );
    // The worker's bridge tells its CLI the one allow, with no updated permissions: only the
    // call's own input back, unchanged, as the bridge gives it for every allow.
    let edit = json!({
        "file_path": edited.display().to_string(), "old_string": "e2e", "new_string": "E2E",
    });
    assert_eq!(
        answer_to(&flow, 2),
        json!({ "behavior": "allow", "updatedInput": edit })
    );
    app.wait_for("the echo", |state| {
        state["echo"] == format!("fix-login allowed Edit in {WORKER}")
    });
    // A second answer to it is refused.
    let (said, error) = bridge.tool_when_free("session_answer", &allow);
    assert!(
        error && said.to_string().contains("answered already"),
        "{said}"
    );

    // The call's row says who allowed it.
    select(&mut app, &format!("*claude: {WORKER}*"));
    app.wait_for("the row", |state| {
        text(state).contains("allowed by fix-login")
    });

    the_person_answers_first(&flow, &mut app, &mut bridge, &first);

    flow.scratch.write("go", b"0");
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
}
