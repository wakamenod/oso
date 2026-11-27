//! A session started outside oso moves into a space at its own request (ADR 0151, step 1).
//! The outside session is a shell script the flow starts itself, not oso: it enters itself in
//! Claude Code's registry as the CLI does, and runs `oso mcp` as its child, as Claude Code runs
//! its MCP servers. The person lets the connection in. A space name that is no open space's is
//! refused with no question. The move into a space of another repository asks, names the
//! repository it adds, and on `y` tells the agent to end its turn. oso then waits, resuming
//! nothing while the outside process runs. Once that ends, as `/exit` would end it, the
//! conversation resumes in oso with step 1's fake `claude`, in the directory it ran in, in that
//! space, which now holds both repositories. Once its entry says its turn ended, its bridge
//! sends the outside session `SIGTERM` itself, and the resume follows (step 2). Killing the
//! waiting buffer instead cancels it: nothing resumes once the outside process ends.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: which registry
//! entry is the asker's, the repository found, the wait's steps and the question's words have
//! their own tests.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::session_tools::{FAKE, run_in, runs};
use super::spaces::{PrivateRoot, answer};
use super::worktrees::{DEADLINE, Flow, asks};

/// The conversation the outside session runs.
const ID: &str = "5678cdef-0000-4000-8000-00000000cccc";

/// The outside session: its registry entry, then `oso mcp` as its child, reading the flow's
/// requests. It runs until the flow writes `exit`, as the person's `/exit` ends it, or until
/// a `SIGTERM`, which only its bridge sends. Once the flow writes `idle`, its entry says the
/// turn ended, as the CLI's does. `DIR` is the flow's scratch directory.
const OUTSIDE: &str = r#"#!/bin/sh
dir='DIR'
mkdir -p "$HOME/.claude/sessions"
entry="$HOME/.claude/sessions/$$.json"
started=$(date +%s)
write() {
  printf '{"pid":%s,"sessionId":"%s","cwd":"%s","startedAt":%s000,"name":"outside-fix","kind":"interactive","status":"%s","statusUpdatedAt":%s000}\n' \
    $$ "$MOVE_ID" "$MOVE_CWD" "$started" "$1" "$(date +%s)" > "$entry.new" && mv "$entry.new" "$entry"
}
trap 'rm -f "$entry"; : > "$dir/terminated"; exit 143' TERM
write busy
"$OSO" mcp --pid "$OSO_PID" --requests "$dir/move-requests.jsonl" > "$dir/move-out" 2> "$dir/move-err" &
while [ ! -f "$dir/exit" ]; do
  [ -d "$dir" ] || break
  if [ -f "$dir/idle" ] && [ ! -f "$dir/idle-written" ]; then
    write idle
    : > "$dir/idle-written"
  fi
  sleep 0.1
done
rm -f "$entry"
"#;

/// How long the app takes no answer to an agent's question after a key, and a moment more.
const QUIET: Duration = Duration::from_millis(700);

/// What the outside session's agent asks: a space that is none, then the space.
fn requests(space: &str) -> String {
    let request = |id: u64, method: &str, params: Value| {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
    };
    let call = |id: u64, space: &str| {
        let arguments = json!({ "space": space });
        let params = json!({ "name": "move_to_space", "arguments": arguments });
        request(id, "tools/call", params)
    };
    let lines = [
        request(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        call(2, "no-such-space"),
        call(3, space),
    ];
    format!("{}\n", lines.join("\n"))
}

/// The text of the bridge's result for request `id`, once it has written it.
fn result(flow: &Flow, id: u64) -> String {
    let path = flow.scratch.path().join("move-out");
    let deadline = Instant::now() + DEADLINE;
    loop {
        let text = fs::read_to_string(&path).unwrap_or_default();
        let found = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|response| response["id"] == id);
        if let Some(response) = found {
            return response["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
        }
        assert!(Instant::now() < deadline, "no result {id}: {text}");
        thread::sleep(Duration::from_millis(100));
    }
}

/// The space named `name` as the dev state lists it.
fn space<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["sessions"]["spaces"]
        .as_array()?
        .iter()
        .find(|space| space["name"] == name)
}

/// The hosted session named `name` as the dev state lists it.
fn session<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["sessions"]["sessions"]
        .as_array()?
        .iter()
        .find(|session| session["name"] == name)
}

/// Waits for the question that starts `start`, and answers it `y` after a quiet moment.
fn allow(app: &mut crate::harness::App, start: &str) -> String {
    let state = app.wait_for(start, |state| {
        asks(state).is_some_and(|question| question.starts_with(start))
    });
    thread::sleep(QUIET);
    app.send_keys("y");
    asks(&state).unwrap_or_default()
}

/// A move under way: the instance, the outside session, the repository it runs in.
struct Moving {
    flow: Flow,
    app: crate::harness::App,
    outside: oso_platform::process::Child,
    repo: std::path::PathBuf,
    /// The spaces' root, kept while the flow runs.
    _root: PrivateRoot,
}

/// Makes a space of another repository, starts the outside session in the flow's trusted
/// repository, lets it in and answers `y` to its move, then waits for oso's waiting buffer.
/// It also checks the refusal of a space that is none, the question's words and the reply.
fn moving(test: &str) -> Moving {
    // The space's directory must have a private way for its orchestrator to start, which
    // `space-new` does: the system's temporary directory is shared on Linux.
    let root = PrivateRoot::new(&format!("{test}-root"));
    let config = format!("\n[spaces]\nroot = \"{}\"\n", root.0.display());
    let flow = Flow::with_fake(test, &config, FAKE);
    // The space's repository, which oso need not trust, and the outside session's, trusted.
    let other = flow.scratch.path().join("other");
    fs::create_dir(&other).unwrap();
    flow.git(&other, &["init", "-q", "-b", "main"]);
    fs::write(other.join("README.md"), "# other\n").unwrap();
    let (mut app, _) = flow.start();
    app.open_file(&other.join("README.md"));
    wait_for_other_project(&mut app);
    app.send_keys("C-c a O");
    answer(&mut app, "Describe the space:", "Fix login error");
    let other_root = fs::canonicalize(&other).unwrap().display().to_string();
    app.wait_for("the space", |state| {
        space(state, "fix-login-error").is_some_and(|s| s["members"] == json!([other_root]))
    });
    // The new space shows its orchestrator, a terminal that takes `C-x`: the file again, so
    // that the flow's keys reach oso.
    app.wait_for("its orchestrator shown", |state| {
        state["buffer"]["name"] == "*claude: fix-login-error*"
    });
    super::spaces::show_readme(&mut app);

    // The person turns the channel on, as an outside session reaches oso only then.
    app.run_command("agent-channel-start", None).unwrap();
    app.wait_for("the channel", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.starts_with("Agent  "))
    });

    // The outside session, in the trusted repository.
    let repo = fs::canonicalize(&flow.repo).unwrap();
    flow.scratch.write(
        "move-requests.jsonl",
        requests("fix-login-error").as_bytes(),
    );
    let outside = start_outside(&flow, &app, &repo);
    allow(&mut app, "Agent: let");
    let refused = result(&flow, 2);
    assert!(refused.contains("no open space is named"), "{refused}");

    let question = allow(&mut app, "Agent: move session");
    assert!(question.contains("\"outside-fix\""), "{question}");
    // The process as the system names it, the shell that runs the script, not as the registry
    // entry says.
    assert!(!question.contains("(\"claude\""), "{question}");
    assert!(question.contains("sh\", pid "), "{question}");
    // A long path keeps its end, which names it.
    let tail = format!("{}/repo\" to it?", test);
    assert!(question.contains(", adding repository \""), "{question}");
    assert!(question.contains(&tail), "{question}");
    let reply = result(&flow, 3);
    assert!(
        reply.contains("\"moving\": true") || reply.contains("\"moving\":true"),
        "{reply}"
    );
    assert!(reply.contains("/exit"), "{reply}");

    let state = app.wait_for("the wait", |state| {
        echo(state).starts_with("Moving \"outside-fix\" into space \"fix-login-error\"")
    });
    let members = &space(&state, "fix-login-error").unwrap()["members"];
    assert_eq!(members.as_array().map(Vec::len), Some(2), "{members}");
    // The space's running orchestrator gets the trusted repository it just took once it shows
    // an empty input box, which the fake never does; nothing is asked (ADR 0185).
    Moving {
        flow,
        app,
        outside,
        repo,
        _root: root,
    }
}

/// What the echo area says.
fn echo(state: &Value) -> &str {
    state["echo"].as_str().unwrap_or_default()
}

/// The person's `/exit` where the outside session runs: it ends, within the flow's deadline.
fn exit_outside(moving: &mut Moving) {
    moving.flow.scratch.write("exit", b"");
    let deadline = Instant::now() + DEADLINE;
    while moving
        .outside
        .try_wait()
        .expect("the outside session")
        .is_none()
    {
        assert!(Instant::now() < deadline, "the outside session did not end");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_started_outside_oso_moves_into_a_space_once_it_stopped_there() {
    let mut moving = moving("space-move");
    // It waits while the outside process runs, and resumes nothing.
    thread::sleep(Duration::from_millis(1500));
    assert!(
        resumes(&moving.flow) == 0,
        "a resume ran while the outside process ran"
    );

    // The person's `/exit` there: it resumes here, in its directory, in the space.
    exit_outside(&mut moving);
    let run = run_in(&moving.flow, &moving.repo);
    assert!(
        run.args.iter().any(|arg| arg == &format!("--resume={ID}")),
        "{:?}",
        run.args
    );
    moving
        .app
        .wait_for("the moved session in its space", |state| {
            session(state, "outside-fix").is_some_and(|s| s["space"] == "Fix login error")
                && echo(state) == "Moved \"outside-fix\" into space \"fix-login-error\""
        });
    moving.flow.scratch.write("go", b"0");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_bridge_stops_its_session_once_its_turn_ended() {
    let mut moving = moving("space-move-stop");
    // The entry's time is whole seconds: one from a second after the reply is after it.
    thread::sleep(Duration::from_millis(1100));
    assert!(
        resumes(&moving.flow) == 0,
        "a resume ran while the outside process ran"
    );
    // Its turn ends: the bridge sends its own parent `SIGTERM`, and nothing else stops it.
    moving.flow.scratch.write("idle", b"");
    let deadline = Instant::now() + DEADLINE;
    while moving
        .outside
        .try_wait()
        .expect("the outside session")
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "the bridge did not stop its session"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        moving.flow.scratch.path().join("terminated").exists(),
        "the outside session ended without a SIGTERM"
    );
    let run = run_in(&moving.flow, &moving.repo);
    assert!(
        run.args.iter().any(|arg| arg == &format!("--resume={ID}")),
        "{:?}",
        run.args
    );
    moving
        .app
        .wait_for("the moved session in its space", |state| {
            session(state, "outside-fix").is_some_and(|s| s["space"] == "Fix login error")
        });
    moving.flow.scratch.write("go", b"0");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn killing_the_waiting_buffer_cancels_the_move() {
    let mut moving = moving("space-move-cancel");
    // The waiting buffer is internal, and `C-x b` and `C-x k` take it as a person's: from a
    // file, as the space's window may show its orchestrator's terminal, which takes `C-x`.
    super::spaces::show_readme(&mut moving.app);
    moving.app.send_keys("C-x b");
    answer(
        &mut moving.app,
        "Switch to buffer:",
        "*moving: outside-fix*",
    );
    moving.app.wait_for("the waiting buffer", |state| {
        state["buffer"]["name"] == "*moving: outside-fix*"
    });
    moving.app.send_keys("C-x k RET");
    moving.app.wait_for("the move cancelled", |state| {
        state["buffer"]["name"] != "*moving: outside-fix*" && echo(state).contains("was cancelled")
    });
    // Its turn ends after the cancel: its bridge stops nothing.
    moving.flow.scratch.write("idle", b"");
    thread::sleep(Duration::from_millis(1500));
    assert!(
        !moving.flow.scratch.path().join("terminated").exists(),
        "a cancelled move stopped the outside session"
    );
    // The outside session ends, and nothing resumes it.
    exit_outside(&mut moving);
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(resumes(&moving.flow), 0, "a cancelled move resumed");
}

/// How many runs of the fake there were but the space's orchestrator's, which `space-new`
/// starts (ADR 0159).
fn resumes(flow: &Flow) -> usize {
    let orchestrator = "--name=fix-login-error";
    (runs(flow).iter())
        .filter(|run| !run.args.iter().any(|arg| arg == orchestrator))
        .count()
}

/// Waits until the project of the buffer just opened is found, so that `space-new` takes it.
/// It stays restricted: a space's repository need not be trusted.
fn wait_for_other_project(app: &mut crate::harness::App) {
    app.wait_until_ready();
    app.wait_for("the other project", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("  other  restricted  "))
    });
}

/// Starts the outside session in `cwd`, outside oso, with the flow's home, so that its registry
/// entry is the one the instance reads.
fn start_outside(
    flow: &Flow,
    app: &crate::harness::App,
    cwd: &Path,
) -> oso_platform::process::Child {
    let script = flow.scratch.write(
        "outside",
        OUTSIDE
            .replace("DIR", &flow.scratch.path().display().to_string())
            .as_bytes(),
    );
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let sh = Executable::absolute("/bin/sh").expect("sh is there");
    let oso = fs::canonicalize(env!("CARGO_BIN_EXE_oso")).expect("the binary is there");
    let pid = app.pid().to_string();
    let args: Vec<OsString> = vec![script.into()];
    let set = [
        ("HOME", flow.home.as_os_str()),
        ("MOVE_ID", OsStr::new(ID)),
        ("MOVE_CWD", cwd.as_os_str()),
        ("OSO", oso.as_os_str()),
        ("OSO_PID", OsStr::new(&pid)),
    ];
    spawn(&Spawn {
        executable: &sh,
        args: &args,
        cwd: Some(cwd),
        project: None,
        inherit: &["XDG_RUNTIME_DIR"],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: false,
        set: &set,
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .expect("cannot start the outside session")
}
