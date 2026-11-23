//! Dictation through TypeWhisper (ADR 0175), against the fake TypeWhisper that `[dictation]
//! discovery` leads to: the text goes into a buffer, the minibuffer and a native session's
//! composer where the dictation started, and undoes in one step; TypeWhisper's own Command-V is
//! dropped while the clipboard carries its marker and runs when it does not; and no discovery
//! file, a refused token, a busy TypeWhisper and one that dies are each said. The dev socket's
//! keys stand in for the person's, and the clipboard is the one in memory.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the discovery
//! file's checks, the curl configuration, the thread's steps and the held key's queue have
//! their own tests.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Child, Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::harness::{App, Scratch};
use super::spaces::trust_restricted;
use super::worktrees::keys;

const ID: &str = "0F3C2B1A-1234-4ABC-9DEF-0123456789AB";

/// What the fake hears.
const SAID: &str = "hello from TypeWhisper";

/// TypeWhisper's marker on the clipboard.
const MARKER: &str = "com.typewhisper.SpeechTranscription";

/// The fake TypeWhisper, running until the flow ends.
struct Fake {
    child: Child,
    discovery: PathBuf,
    requests: PathBuf,
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Fake {
    /// Starts the fake with `script`, and waits for its discovery file.
    fn start(scratch: &Scratch, script: &Value) -> Self {
        let script = scratch.write("typewhisper/script.json", script.to_string().as_bytes());
        let dir = scratch.path().join("typewhisper");
        let (discovery, requests) = (dir.join("api-discovery.json"), dir.join("requests.jsonl"));
        let fake = Executable::absolute(env!("CARGO_BIN_EXE_fake-typewhisper")).unwrap();
        let args = [
            script.into_os_string(),
            discovery.clone().into(),
            requests.clone().into(),
        ];
        let child = spawn(&Spawn {
            executable: &fake,
            args: &args,
            cwd: None,
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            own_session: false,
            set: &[],
            terminal: None,
            pass_fd: None,
            args_logged: true,
        })
        .unwrap();
        let fake = Self {
            child,
            discovery,
            requests,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !fake.discovery.exists() {
            assert!(
                Instant::now() < deadline,
                "the fake wrote no discovery file"
            );
            thread::sleep(Duration::from_millis(20));
        }
        fake
    }

    /// The requests it was asked, as `METHOD path` with whether they carried the token.
    fn requests(&self) -> Vec<(String, bool, String)> {
        let text = fs::read_to_string(&self.requests).unwrap_or_default();
        text.lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                (
                    format!(
                        "{} {}",
                        value["method"].as_str().unwrap(),
                        value["path"].as_str().unwrap()
                    ),
                    value["authorized"] == true,
                    value["body"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect()
    }
}

/// A script that records, then gives `SAID` once it is stopped, after `processing` looks that
/// say it is still at work.
fn script(processing: usize) -> Value {
    let mut transcription: Vec<Value> = (0..processing)
        .map(|_| json!({"status": 200, "body": {"id": ID, "status": "processing"}}))
        .collect();
    transcription.push(json!({"status": 200, "body": {
        "id": ID,
        "status": "completed",
        "transcription": {"text": SAID, "raw_text": SAID},
    }}));
    json!({
        "routes": {
            "POST /v1/dictation/start": [{"status": 200, "body": {"id": ID, "status": "recording"}}],
            "POST /v1/dictation/stop": [{"status": 200, "body": {"id": ID, "status": "stopped"}}],
            "GET /v1/dictation/transcription": transcription,
        }
    })
}

/// A configuration whose `[dictation] discovery` is `discovery`, with `more` after it.
fn write_config(scratch: &Scratch, discovery: &Path, more: &str) -> PathBuf {
    let text = format!(
        "[lsp]\nstart = \"manual\"\n\n{more}\n[dictation]\ndiscovery = \"{}\"\n",
        discovery.display()
    );
    let config = scratch.write("conf/config.toml", text.as_bytes());
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    config
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// The app visiting `file` with `config`, its state and home in the scratch directory
/// (ADR 0146).
fn start(scratch: &Scratch, config: &Path, file: &Path) -> App {
    let state = scratch.path().join("state");
    let home = scratch.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let env: [(&str, &OsStr); 2] = [
        ("OSO_STATE_DIR", state.as_os_str()),
        ("HOME", home.as_os_str()),
    ];
    let mut app = App::start_with_file_config_and_env(file, scratch.path(), config, &env);
    app.wait_until_ready();
    app
}

/// The selected window's rows in view, joined by line breaks.
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

fn modeline_has(app: &mut App, what: &str, part: &'static str) -> Value {
    app.wait_for(what, move |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains(part))
    })
}

fn echo_has(app: &mut App, what: &str, part: &'static str) -> Value {
    app.wait_for(what, move |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains(part))
    })
}

/// Starts a dictation with `s-d`, waits for `● REC`, and stops it.
fn dictate(app: &mut App) {
    app.send_keys("s-d");
    modeline_has(app, "the dictation recording", "● REC");
    app.send_keys("s-d");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_dictation_types_into_the_buffer_where_it_started_and_undoes_in_one_step() {
    let scratch = Scratch::new("dictation-buffer");
    let fake = Fake::start(&scratch, &script(2));
    let config = write_config(&scratch, &fake.discovery, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    dictate(&mut app);
    let state = app.wait_for("the text", |state| text(state).contains(SAID));
    assert!(
        text(&state).starts_with(&format!("{SAID}start")),
        "{}",
        text(&state)
    );
    assert!(!state["modeline"].as_str().unwrap().contains("REC"));
    assert!(!state["modeline"].as_str().unwrap().contains('…'));
    app.send_keys("C-/");
    app.wait_for("one undo", |state| text(state).starts_with("start"));

    let requests = fake.requests();
    assert!(
        requests.iter().all(|(_, authorized, _)| *authorized),
        "{requests:?}"
    );
    assert_eq!(requests[0].0, "POST /v1/dictation/start");
    assert_eq!(requests[0].2, "", "no workflow, no body");
    assert!(
        requests
            .iter()
            .any(|(request, ..)| request == "POST /v1/dictation/stop")
    );
    assert!(
        requests
            .iter()
            .any(|(request, ..)| *request == format!("GET /v1/dictation/transcription?id={ID}"))
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_dictation_types_into_the_minibuffer_it_started_in() {
    let scratch = Scratch::new("dictation-minibuffer");
    let fake = Fake::start(&scratch, &script(0));
    let config = write_config(&scratch, &fake.discovery, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.send_keys("M-x");
    app.wait_for("M-x's prompt", |state| {
        state["prompts"][0]["prompt"].is_string()
    });
    dictate(&mut app);
    app.wait_for("the text in the minibuffer", |state| {
        state["prompts"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains(SAID))
    });
    app.send_keys("C-g");
    let state = app.state();
    assert!(
        text(&state).starts_with("start"),
        "the buffer beneath is untouched"
    );
    assert!(!text(&state).contains(SAID));
}

/// A fake `claude -p` that reads its input and never answers.
const CLAUDE: &str = "#!/bin/sh\nwhile IFS= read -r line; do :; done\n";

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_dictation_types_into_a_native_sessions_composer() {
    let scratch = Scratch::new("dictation-composer");
    let fake = Fake::start(&scratch, &script(0));
    let claude = scratch.write("fake-claude", CLAUDE.as_bytes());
    set_mode(&claude, 0o700);
    let more = format!(
        "[terminal]\nshell = \"/bin/sh\"\nlogin = false\n\n[sessions]\nclaude = \"{}\"\n",
        claude.display()
    );
    let config = write_config(&scratch, &fake.discovery, &more);
    let project = scratch.path().join("proj");
    fs::create_dir(&project).unwrap();
    git_init(&project);
    let file = project.join("notes.md");
    fs::write(&file, "notes\n").unwrap();
    let mut app = start(&scratch, &config, &file);
    trust_restricted(&mut app, "proj");
    app.wait_for("the project trusted", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("restricted"))
    });
    app.run_command("claude", None).unwrap();
    app.wait_for("the composer", in_composer);
    app.send_keys(&keys("ask: "));
    dictate(&mut app);
    app.wait_for("the text in the composer", |state| {
        in_composer(state) && text(state) == format!("ask: {SAID}")
    });
}

/// Whether the selected window is the composer docked under a session.
fn in_composer(state: &Value) -> bool {
    state["windows"].as_array().is_some_and(|windows| {
        windows
            .iter()
            .any(|window| window["selected"] == true && window["docked"]["slot"] == "composer")
    })
}

/// `git init` in `dir`, so that it is a project oso finds.
fn git_init(dir: &Path) {
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

fn top(state: &Value) -> u64 {
    state["scroll"]["top"].as_u64().unwrap()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn typewhispers_command_v_is_dropped_with_its_marker_and_the_persons_m_v_runs() {
    let scratch = Scratch::new("dictation-paste");
    // Looks that keep it at work for about 12 s once stopped, while the guard is open.
    let fake = Fake::start(&scratch, &script(40));
    let config = write_config(&scratch, &fake.discovery, "");
    let lines: String = (0..200).map(|n| format!("line {n}\n")).collect();
    let file = scratch.write("lines.txt", lines.as_bytes());
    let mut app = start(&scratch, &config, &file);
    app.send_keys("M->");
    dictate(&mut app);
    modeline_has(&mut app, "the dictation waiting for its text", "…");
    let before = top(&app.state());

    // TypeWhisper's paste: the clipboard carries its marker, and Command-V comes.
    app.call("clipboard", Some(json!({"text": SAID, "types": [MARKER]})));
    app.send_keys("M-v a");
    let state = app.wait_for("the paste looked at", |state| state["paste_held"] == false);
    assert_eq!(top(&state), before, "Command-V did not scroll");
    assert!(
        text(&state).contains("line 199\na"),
        "the key after it ran: {}",
        text(&state)
    );

    // The person's own Command-V, over another app's copy: it scrolls.
    app.call("clipboard", Some(json!({"text": "copied"})));
    app.send_keys("M-v");
    let state = app.wait_for("the person's M-v", |state| state["paste_held"] == false);
    assert!(top(&state) < before, "M-v scrolled");
    assert!(
        state["modeline"].as_str().unwrap().contains('…'),
        "both while the text was due"
    );
    // The key typed after the paste changed the text, so the dictation goes to the kill ring.
    echo_has(
        &mut app,
        "the text kept",
        "Dictation kept in the kill ring: its text changed",
    );
    app.send_keys("C-y");
    app.wait_for("the yank", |state| {
        text(state).contains(&format!("a{SAID}"))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn no_discovery_file_says_typewhisper_is_not_running() {
    let scratch = Scratch::new("dictation-missing");
    let config = write_config(
        &scratch,
        &scratch.path().join("none/api-discovery.json"),
        "",
    );
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.send_keys("s-d");
    echo_has(
        &mut app,
        "the message",
        "TypeWhisper isn't running, or is its API server on?",
    );
    assert!(!app.state()["modeline"].as_str().unwrap().contains("REC"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_refused_token_and_a_busy_typewhisper_are_said() {
    let scratch = Scratch::new("dictation-401");
    let mut refused = script(0);
    refused["discovery_token"] = json!("another-token-0123456789");
    let fake = Fake::start(&scratch, &refused);
    let config = write_config(&scratch, &fake.discovery, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.send_keys("s-d");
    echo_has(&mut app, "the refusal", "TypeWhisper refused oso's token");
    assert_eq!(fake.requests().len(), 1, "never tried again");
    drop(app);
    drop(fake);

    let scratch = Scratch::new("dictation-409");
    let mut busy = script(0);
    busy["routes"]["POST /v1/dictation/start"] = json!([{"status": 409, "body": {
        "error": {"code": "conflict", "message": "Dictation is not idle"}
    }}]);
    let fake = Fake::start(&scratch, &busy);
    let config = write_config(&scratch, &fake.discovery, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.send_keys("s-d");
    echo_has(
        &mut app,
        "the conflict",
        "TypeWhisper can't dictate now: Dictation is not idle",
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn typewhisper_that_dies_while_recording_is_said() {
    let scratch = Scratch::new("dictation-dies");
    let mut dies = script(0);
    // The start and one look, then it is gone.
    dies["exit_after"] = json!(2);
    let fake = Fake::start(&scratch, &dies);
    let config = write_config(&scratch, &fake.discovery, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.send_keys("s-d");
    modeline_has(&mut app, "the dictation recording", "● REC");
    let state = echo_has(&mut app, "the end", "TypeWhisper stopped answering");
    assert!(!state["modeline"].as_str().unwrap().contains("REC"));
    assert!(!text(&state).contains(SAID));
}
