//! Adopting a Claude Code session started outside oso (ADR 0121, "Adopting a session started
//! outside oso"), with step 1's fake `claude`: a conversation that runs elsewhere is refused
//! until it stops, one that stopped is resumed in its own directory under its own name, and
//! both are remembered as oso's own.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: what is read of a
//! transcript, the candidates and their names have their own tests.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::{Flow, asks, ended, now_ms, shows};
use crate::harness::App;

/// A conversation that stopped, named by `/rename`, and one whose process runs elsewhere.
const STOPPED: &str = "1234abcd-0000-4000-8000-00000000aaaa";
const LIVE: &str = "5678cdef-0000-4000-8000-00000000bbbb";

/// Writes the transcript of `id`, which ran in `cwd`, as Claude Code keeps it.
fn transcript(home: &Path, id: &str, cwd: &Path, title: Option<&str>) {
    let dir = home.join(".claude/projects/-elsewhere");
    fs::create_dir_all(&dir).unwrap();
    let mut text = String::new();
    if let Some(title) = title {
        // As the CLI writes it, `type` first: `json!` would sort the keys.
        text.push_str(&format!(
            "{{\"type\":\"custom-title\",\"customTitle\":\"{title}\",\"sessionId\":\"{id}\"}}\n"
        ));
    }
    let user = json!({
        "parentUuid": null,
        "message": { "role": "user", "content": "never read" },
        "cwd": cwd,
        "sessionId": id,
    });
    text.push_str(&user.to_string());
    text.push('\n');
    fs::write(dir.join(format!("{id}.jsonl")), text).unwrap();
}

/// The rows of the open prompt's completion.
fn rows(state: &Value) -> Vec<String> {
    state["prompts"][0]["completion"]["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Opens `session-adopt` once its list holds `name`, and picks it.
fn adopt(app: &mut App, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.send_keys("C-x a a");
        let state = app.wait_for("the prompt", |state| {
            state["prompts"][0]["completion"].is_object()
        });
        let rows = rows(&state);
        if let Some(at) = rows.iter().position(|row| row == name) {
            for _ in 0..at {
                app.send_keys("C-n");
            }
            app.send_keys("RET");
            return;
        }
        assert!(Instant::now() < deadline, "{name} is not listed: {rows:?}");
        app.send_keys("C-g");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The directory the fake's run with `--resume=id` ran in.
fn run_directory(flow: &Flow, id: &str) -> String {
    let wanted = format!("arg --resume={id}");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for entry in fs::read_dir(flow.scratch.path()).unwrap().flatten() {
            if !entry.file_name().to_string_lossy().starts_with("run.") {
                continue;
            }
            let text = fs::read_to_string(entry.path()).unwrap_or_default();
            if text.lines().any(|line| line == wanted)
                && let Some(cwd) = text.lines().find_map(|line| line.strip_prefix("cwd "))
            {
                return cwd.to_owned();
            }
        }
        assert!(Instant::now() < deadline, "no run of {id}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A process that stands for `claude` running conversation `id` in another program, in `cwd`,
/// named `name`, with its registry entry.
fn run_elsewhere(flow: &Flow, id: &str, cwd: &Path, name: &str) -> std::process::Child {
    let other = spawn(&Spawn {
        executable: &Executable::absolute("/bin/sleep").unwrap(),
        args: &["60".into()],
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
        "cwd": cwd,
        "startedAt": now_ms(),
        "name": name,
        "status": "busy",
    });
    fs::write(
        sessions.join(format!("{}.json", other.id())),
        entry.to_string(),
    )
    .unwrap();
    other
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_started_elsewhere_is_adopted_once_it_stops_and_stays_osos() {
    let flow = Flow::new("sessions-adopt");
    let elsewhere = flow.scratch.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let elsewhere = elsewhere.canonicalize().unwrap();
    transcript(&flow.home, STOPPED, &elsewhere, Some("fix-tests"));
    transcript(&flow.home, LIVE, &elsewhere, None);
    // The live one runs in another program: a process with its registry entry.
    let mut other = run_elsewhere(&flow, LIVE, &elsewhere, "live-one");
    flow.hold();
    let mut app = flow.start();

    // While it runs, it is refused, and nothing starts.
    adopt(&mut app, "live-one");
    let state = app.wait_for("the refusal", |state| asks(state).is_some());
    let question = asks(&state).unwrap();
    assert!(
        question.contains(&format!("process {}", other.id()))
            && question.contains("stop it there")
            && question.contains("fork it"),
        "{question}"
    );
    app.send_keys("n");
    assert!(flow.runs().is_empty(), "nothing ran: {:?}", flow.runs());

    // Stopped where it ran, it is resumed in its directory under the name it had there. The
    // list still says it runs: the start thread looks again.
    let _ = other.kill();
    let _ = other.wait();
    adopt(&mut app, "live-one");
    assert_eq!(run_directory(&flow, LIVE), elsewhere.display().to_string());
    let (args, session) = flow.wait_for_run(&format!("--resume={LIVE}"));
    assert_eq!(session, LIVE);
    assert_eq!(super::flag(&args, "--name="), Some("live-one"));
    app.wait_for("the adopted buffer", |state| {
        shows(state, "*claude: live-one*", "fake claude live-one")
    });

    // A stopped one is resumed by its name.
    adopt(&mut app, "fix-tests");
    assert_eq!(
        run_directory(&flow, STOPPED),
        elsewhere.display().to_string()
    );
    app.wait_for("the second adopted buffer", |state| {
        shows(state, "*claude: fix-tests*", "fake claude fix-tests")
    });

    // Both are oso's now: they come back stopped at the next start.
    flow.exit_with(0);
    app.wait_for("their stop", |state| {
        ended(state, "*claude: live-one*").is_some()
            && ended(state, "*claude: fix-tests*").is_some()
    });
    // The record is written on a thread: give it a moment before the exit.
    std::thread::sleep(Duration::from_millis(500));
    super::quit(&mut app);
    flow.forget_runs();
    let mut app = flow.start();
    app.wait_for("both back stopped", |state| {
        ["*claude: live-one*", "*claude: fix-tests*"]
            .iter()
            .all(|name| {
                ended(state, name).as_deref() == Some("stopped: session-resume to continue")
            })
    });
    assert!(flow.runs().is_empty());
    super::quit(&mut app);
}
