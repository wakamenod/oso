//! A TUI session carried over `oso-restart` and switched to native (ADR 0165, part 4d-2), whose
//! `claude -p` asks the person's permission: its bridge is the new instance's own child's, and
//! is let in as any native session's is, not judged by when the carried TUI started.
//!
//! Must not depend on: the app's internals. Driven by its control socket and the fake alone.

use std::fs;

use serde_json::{Value, json};

use super::super::super::harness::{App, sockets};
use super::super::super::spaces::trust_restricted;
use super::super::super::worktrees::keys;
use super::switch::{IDLE, switch_back, tui_idle};
use super::{Flow, composer_docked, request};

/// The fake: run `N` from 1 logs its arguments to `run.N`. With `-p`, it prints `stream.jsonl`
/// and asks its bridge, `oso mcp --pid PID --permission` as its `--mcp-config` names it, about
/// call `t1`, writing the answer to `answer`; it holds the bridge until the flow writes
/// `answered`, then reads its input to its end. Else it is a TUI that shows `idle.txt` until it
/// is hung up.
const CARRIED_FAKE: &str = r#"#!/bin/sh
dir='DIR'
n=$(ls "$dir" | grep -c '^run\.')
n=$((n+1))
for a in "$@"; do printf 'arg %s\n' "$a"; done > "$dir/tmp.$n"
mv "$dir/tmp.$n" "$dir/run.$n"
if [ "$1" = -p ]; then
  cat "$dir/stream.jsonl"
  oso=$(sed -n 's/.*"command":"\([^"]*\)".*/\1/p' "$dir/run.$n" | head -n 1)
  pid=$(sed -n 's/.*"--pid","\([0-9]*\)".*/\1/p' "$dir/run.$n" | head -n 1)
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"permission","arguments":{"tool_name":"Bash","input":{"command":"make"},"tool_use_id":"t1"}}}'
    while [ ! -f "$dir/answered" ]; do sleep 0.1; done
  } | "$oso" mcp --pid "$pid" --permission | {
    IFS= read -r init
    IFS= read -r line
    printf '%s\n' "$line" > "$dir/tmp.answer"
    mv "$dir/tmp.answer" "$dir/answer"
  }
  while IFS= read -r line; do :; done
  exit 0
fi
h=
trap 'h=1' HUP
printf '\033[2J\033[H'
cat "$dir/idle.txt"
while [ -z "$h" ]; do sleep 0.1; done
exit 0
"#;

/// What the `-p` fake prints: its start, and the call it asks about.
fn stream() -> String {
    let lines = [
        json!({"type": "system", "subtype": "init", "cwd": "/w", "session_id": "s", "model": "m"}),
        json!({"type": "assistant", "message": {"id": "m1", "content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "make"}},
        ]}}),
    ];
    let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
    format!("{}\n", lines.join("\n"))
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_tui_carried_over_a_restart_and_switched_to_native_asks_the_person() {
    let flow = Flow::with("native-carried-asks", CARRIED_FAKE, &stream());
    flow.scratch.write("idle.txt", IDLE.as_bytes());
    // A TUI session, as one started before native sessions were the default.
    let config = fs::read_to_string(&flow.config).unwrap();
    let tui = config.replacen("[sessions]\n", "[sessions]\nmode = \"tui\"\n", 1);
    assert_ne!(tui, config, "the flow's config has a [sessions] table");
    fs::write(&flow.config, tui).unwrap();
    let mut old = flow.start();
    old.open_file(&flow.project.join("notes.md"));
    trust_restricted(&mut old, "proj");
    old.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });
    old.run_command("claude", None).unwrap();
    tui_idle(&mut old);

    // Its `claude` is carried over the restart, as the new instance's child no longer.
    let before = sockets();
    old.run_command("oso-restart", None)
        .expect("oso-restart runs");
    let mut app = App::started_by_a_restart(&before);
    assert!(old.wait_for_exit().success(), "the instance going exits");
    app.wait_until_ready();
    app.send_keys("C-x b");
    app.send_keys(&keys("*claude: proj*"));
    app.send_keys("RET");
    tui_idle(&mut app);
    // The new instance takes over for a moment after it draws, when no session switches. The
    // channel its sessions want, off since the restart, comes on once that is over.
    app.wait_for("the takeover's end", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.starts_with("Agent  "))
    });

    // Switched to native: `claude -p` is the new instance's own child, and its bridge asks.
    switch_back(&mut app);
    let native = flow.wait_for_file("run.2");
    assert!(native.lines().any(|line| line == "arg -p"), "{native}");
    request::request(&mut app, "the block");
    let state = app.state();
    let shown = state["text"].to_string();
    assert!(shown.contains("$ make"), "{shown}");
    request::go(&mut app);
    request::allow(&mut app);
    let answer: Value = serde_json::from_str(&flow.wait_for_file("answer")).unwrap();
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    let said: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        said,
        json!({"behavior": "allow", "updatedInput": {"command": "make"}})
    );
    // Answered, point goes to the composer, docked under the transcript as a native start's.
    app.wait_for("the composer docked", composer_docked);
    flow.scratch.write("answered", b"");
    let _ = app.exchange("exit", None);
}
