//! Hosted agent sessions (ADR 0121, step 1), with a fake `claude`: a shell script that
//! `[sessions] claude` names, which writes its arguments and `OSO_SESSION` to a file, enters
//! itself in Claude Code's registry as the CLI does, runs the bridge from its `--mcp-config`
//! when the flow asks, and exits with the status the flow gives it. The instance's `HOME` is
//! the flow's own, so the registry is too, and no startup file of the person's runs.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the command
//! line, the wrapper, the registry reader and the admission rule have their own tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::harness::{App, Scratch};

/// The fake: what it was given, its registry entry, the bridge, then the exit the flow
/// writes to `go`. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
{
  for a in "$@"; do printf 'arg %s\n' "$a"; done
  printf 'session %s\n' "$OSO_SESSION"
  printf 'cwd %s\n' "$PWD"
} > "$dir/run.$$"
id=''; name=''; cfg=''
for a in "$@"; do
  case "$a" in
    --session-id=*) id=${a#--session-id=} ;;
    --resume=*) [ -z "$id" ] && id=${a#--resume=} ;;
    --name=*) name=${a#--name=} ;;
    --mcp-config=*) cfg=${a#--mcp-config=} ;;
  esac
done
mkdir -p "$HOME/.claude/sessions"
entry="$HOME/.claude/sessions/$$.json"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","startedAt":%s000,"name":"%s","kind":"interactive","status":"idle"}\n' \
  $$ "$id" "$PWD" "$(date +%s)" "$name" > "$entry"
trap 'rm -f "$entry"' EXIT
printf 'fake claude %s\n' "$name"
if [ -f "$dir/bridge" ]; then
  oso=$(printf '%s' "$cfg" | sed -n 's/.*"command":"\([^"]*\)".*/\1/p')
  pid=$(printf '%s' "$cfg" | sed -n 's/.*"--pid","\([0-9]*\)".*/\1/p')
  "$oso" mcp --pid "$pid" --requests "$dir/requests.jsonl" > "$dir/mcp.$$" 2>/dev/null
  printf 'bridge done\n'
fi
while [ ! -f "$dir/go" ]; do sleep 0.1; done
exit "$(cat "$dir/go")"
"#;

/// A flow's files: the scratch directory, the project the instance starts in, and its home.
struct Flow {
    scratch: Scratch,
    project: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl Flow {
    fn new(test: &str) -> Flow {
        let scratch = Scratch::new(test);
        let project = scratch.path().join("proj");
        let home = scratch.path().join("home");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&home).unwrap();
        let fake = scratch.write(
            "fake-claude",
            FAKE.replace("DIR", &scratch.path().display().to_string())
                .as_bytes(),
        );
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let config = format!(
            "[terminal]\nshell = \"/bin/sh\"\nlogin = false\n\n[sessions]\nclaude = \"{}\"\n",
            fake.display()
        );
        let config = scratch.write("config.toml", config.as_bytes());
        scratch.write("requests.jsonl", requests().as_bytes());
        Flow {
            scratch,
            project,
            home,
            config,
        }
    }

    fn start(&self) -> App {
        let mut app = App::start_with_config_and_env(
            &self.project,
            &self.config,
            &[("HOME", self.home.as_os_str())],
        );
        app.wait_until_ready();
        app
    }

    fn path(&self, name: &str) -> PathBuf {
        self.scratch.path().join(name)
    }

    /// The fake's next run exits with `code` once it is there.
    fn exit_with(&self, code: u8) {
        self.scratch.write("go", code.to_string().as_bytes());
    }

    /// The fake's next run waits.
    fn hold(&self) {
        let _ = fs::remove_file(self.path("go"));
    }

    /// Each run of the fake: the arguments it was given, and its `OSO_SESSION`.
    fn runs(&self) -> Vec<(Vec<String>, String)> {
        let mut runs: Vec<(Vec<String>, String)> = fs::read_dir(self.scratch.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("run."))
            .map(|entry| {
                let text = fs::read_to_string(entry.path()).unwrap_or_default();
                let args = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("arg "))
                    .map(str::to_owned)
                    .collect();
                let session = text
                    .lines()
                    .find_map(|line| line.strip_prefix("session "))
                    .unwrap_or_default()
                    .to_owned();
                (args, session)
            })
            .collect();
        runs.sort();
        runs
    }

    fn forget_runs(&self) {
        for entry in fs::read_dir(self.scratch.path()).unwrap().flatten() {
            if entry.file_name().to_string_lossy().starts_with("run.") {
                fs::remove_file(entry.path()).unwrap();
            }
        }
    }

    /// Waits for a run whose arguments hold `wanted`.
    fn wait_for_run(&self, wanted: &str) -> (Vec<String>, String) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(run) = self
                .runs()
                .into_iter()
                .find(|(args, _)| args.iter().any(|arg| arg == wanted))
            {
                return run;
            }
            assert!(
                Instant::now() < deadline,
                "no run with {wanted}: {:?}",
                self.runs()
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// What the fake's bridge asks: it starts, then reads the state.
fn requests() -> String {
    let request = |id: u64, method: &str, params: Value| {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
    };
    let lines = [
        request(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        request(2, "tools/call", json!({ "name": "state", "arguments": {} })),
    ];
    format!("{}\n", lines.join("\n"))
}

/// The terminal of the buffer named `name`.
fn terminal<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["terminals"]
        .as_array()?
        .iter()
        .find(|terminal| terminal["name"] == name)
}

fn shows(state: &Value, name: &str, text: &str) -> bool {
    terminal(state, name).is_some_and(|terminal| {
        terminal["rows"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row.as_str() == Some(text)))
    })
}

fn ended(state: &Value, name: &str) -> Option<String> {
    terminal(state, name).and_then(|terminal| terminal["ended"].as_str().map(str::to_owned))
}

fn asks(state: &Value) -> Option<String> {
    state["prompts"].as_array()?.first()?["prompt"]
        .as_str()
        .map(str::to_owned)
}

/// The value of the run's `--flag=`.
fn flag<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().find_map(|arg| arg.strip_prefix(flag))
}

/// The UNIX time now, in ms.
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_starts_its_bridge_gets_in_unasked_and_it_resumes_or_forks() {
    let flow = Flow::new("sessions-start");
    let mut app = flow.start();
    let id = start_with_a_bridge(&flow, &mut app);
    fail_then_resume(&flow, &mut app, &id);
    let new = fork_when_live(&flow, &mut app, &id);
    // A bridge from elsewhere that claims the session is asked about.
    flow.exit_with(0);
    app.wait_for("the fork's stop", |state| {
        ended(state, "*claude: proj-2*").is_some()
    });
    let mut outside = app.mcp_claiming(&flow.path("requests.jsonl"), Some(&new));
    let state = app.wait_for("the question", |state| asks(state).is_some());
    let question = asks(&state).unwrap();
    assert!(question.starts_with("Agent: let"), "{question}");
    let _ = outside.kill();
    let _ = outside.wait();
}

/// `claude` makes the buffer, turns the channel on and starts the fake in it, whose bridge
/// is let in without a question, as the session's. Returns the session's id.
fn start_with_a_bridge(flow: &Flow, app: &mut App) -> String {
    flow.scratch.write("bridge", b"");
    // `claude` makes the buffer, turns the channel on, and starts the fake in it.
    app.run_command("claude", None).unwrap();
    let state = app.wait_for("the fake's bridge", |state| {
        shows(state, "*claude: proj*", "bridge done")
    });
    assert_eq!(state["buffer"]["name"], "*claude: proj*");
    assert!(
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.starts_with("Agent")),
        "{}",
        state["modeline"]
    );
    assert!(shows(&state, "*claude: proj*", "fake claude proj"));
    let (args, session) = flow.wait_for_run("--name=proj");
    let id = flag(&args, "--session-id=").unwrap().to_owned();
    assert_eq!(session, id, "OSO_SESSION is the session's id");
    assert!(
        args.iter()
            .all(|arg| ["--session-id=", "--name=", "--mcp-config="]
                .iter()
                .any(|known| arg.starts_with(known))),
        "{args:?}"
    );
    // Its bridge was let in without a question, as the session's.
    assert_eq!(asks(&state), None);
    let answered = fs::read_dir(flow.scratch.path())
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("mcp."))
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect::<String>();
    assert!(
        answered
            .lines()
            .any(|line| line.contains(r#""id":2"#) && line.contains(r#""result""#)),
        "{answered}"
    );
    let _ = fs::remove_file(flow.path("bridge"));
    id
}

/// A failure says so, and how to go on; `session-resume` resumes it in the same buffer.
fn fail_then_resume(flow: &Flow, app: &mut App, id: &str) {
    flow.exit_with(3);
    app.wait_for("the failure", |state| {
        ended(state, "*claude: proj*").is_some_and(|ended| ended.starts_with("failed (exited 3)"))
    });

    // session-resume resumes it in the same buffer.
    flow.hold();
    flow.forget_runs();
    app.run_command("session-resume", None).unwrap();
    let (args, _) = flow.wait_for_run(&format!("--resume={id}"));
    assert!(flag(&args, "--session-id=").is_none(), "{args:?}");
    app.wait_for("the resumed fake", |state| {
        shows(state, "*claude: proj*", "fake claude proj")
    });
    flow.exit_with(0);
    let state = app.wait_for("the stop", |state| {
        ended(state, "*claude: proj*").as_deref() == Some("stopped: session-resume to continue")
    });
    assert_eq!(
        state["terminals"].as_array().map(Vec::len),
        Some(1),
        "the same buffer"
    );
}

/// While another process runs the conversation, a resume is refused, and y forks it. Returns
/// the fork's id.
fn fork_when_live(flow: &Flow, app: &mut App, id: &str) -> String {
    let mut other = spawn(&Spawn {
        executable: &Executable::absolute("/bin/sleep").unwrap(),
        args: &["30".into()],
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
    let sessions = flow.home.join(".claude/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let entry = json!({
        "pid": other.id(),
        "sessionId": id,
        "cwd": "/elsewhere",
        "startedAt": now_ms(),
        "name": "proj",
    });
    fs::write(
        sessions.join(format!("{}.json", other.id())),
        entry.to_string(),
    )
    .unwrap();
    flow.hold();
    flow.forget_runs();
    app.run_command("session-resume", None).unwrap();
    let state = app.wait_for("the refusal", |state| asks(state).is_some());
    let question = asks(&state).unwrap();
    assert!(
        question.contains(&format!("process {}", other.id())) && question.contains("fork it"),
        "{question}"
    );
    assert!(flow.runs().is_empty(), "nothing ran");
    app.send_keys("y");
    let (args, session) = flow.wait_for_run(&format!("--resume={id}"));
    assert!(args.iter().any(|arg| arg == "--fork-session"), "{args:?}");
    let new = flag(&args, "--session-id=").unwrap();
    assert_ne!(new, id);
    assert_eq!(session, new);
    assert_eq!(flag(&args, "--name="), Some("proj-2"));
    app.wait_for("the fork's buffer", |state| {
        shows(state, "*claude: proj-2*", "fake claude proj-2")
    });
    let _ = other.kill();
    let _ = other.wait();
    new.to_owned()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_start_brings_sessions_back_stopped_and_a_killed_one_stays_archived() {
    let flow = Flow::new("sessions-restore");
    let mut app = flow.start();
    flow.exit_with(0);
    app.run_command("claude", None).unwrap();
    app.wait_for("the stop", |state| {
        ended(state, "*claude: proj*").as_deref() == Some("stopped: session-resume to continue")
    });
    flow.wait_for_run("--name=proj");
    quit(&mut app);

    // Back, stopped, and nothing ran.
    flow.forget_runs();
    let mut app = flow.start();
    let state = app.wait_for("the session back", |state| {
        terminal(state, "*claude: proj*").is_some()
    });
    assert_eq!(
        ended(&state, "*claude: proj*").as_deref(),
        Some("stopped: session-resume to continue")
    );
    thread::sleep(Duration::from_secs(1));
    assert!(flow.runs().is_empty(), "the fake ran: {:?}", flow.runs());

    // C-x k archives it: the next start does not bring it back.
    app.send_keys("C-x b");
    for key in [
        "*", "c", "l", "a", "u", "d", "e", ":", "SPC", "p", "r", "o", "j", "*",
    ] {
        app.send_keys(key);
    }
    app.send_keys("RET");
    app.wait_for("its buffer", |state| {
        state["buffer"]["name"] == "*claude: proj*"
    });
    app.send_keys("C-x k RET");
    app.wait_for("no session buffer", |state| {
        terminal(state, "*claude: proj*").is_none()
    });
    // The record is written on a thread: give it a moment before the exit.
    thread::sleep(Duration::from_millis(500));
    quit(&mut app);
    let mut app = flow.start();
    thread::sleep(Duration::from_secs(1));
    let state = app.state();
    assert!(terminal(&state, "*claude: proj*").is_none(), "{state}");
    assert!(flow.runs().is_empty());
}

/// Exits the instance with `C-x C-c`, which asks nothing here.
fn quit(app: &mut App) {
    app.send_keys("C-x C-c");
    let status = app.wait_for_exit();
    assert!(status.success(), "{status:?}");
}

mod adopt;
mod attention;
