//! Dictation through TypeWhisper (ADR 0175), against the fake TypeWhisper that `[dictation]
//! discovery` leads to: the text goes into a buffer, the minibuffer and a native session's
//! composer where the dictation started, and undoes in one step; TypeWhisper's own Command-V is
//! dropped while the clipboard carries its marker and runs when it does not; and no discovery
//! file, a refused token, a busy TypeWhisper and one that dies are each said. The dev socket's
//! keys stand in for the person's, and the clipboard is the one in memory. Step 2's fix runs a
//! fake `claude` from `[sessions] claude`, in every flow, so that no real one is ever run: the
//! text shows provisional, then fixed, and one undo brings back what was said; `C-u s-d` and a
//! buffer that may hold secrets never run it; a slow one is killed at its limit; an edited span
//! is skipped; and the face is drawn only while the fix runs.
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

/// What the fake `claude` answers when it finds nothing to fix.
fn no_fixes() -> Value {
    json!({"type": "result", "is_error": false, "structured_output": {"fixes": []}})
}

/// A fake `claude -p` for a fix, in `scratch`'s `claude` directory: it keeps its arguments and
/// its input there, waits `wait` seconds in a `sleep` whose id it keeps, and answers `answer`.
fn fake_claude(scratch: &Scratch, wait: u32, answer: &Value) -> PathBuf {
    let dir = scratch.path().join("claude");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("answer.json"), answer.to_string()).unwrap();
    let dir = dir.display();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{dir}/args'\ncat > '{dir}/input'\n\
         sleep {wait} &\necho $! > '{dir}/sleep'\nwait\ncat '{dir}/answer.json'\n"
    );
    let claude = scratch.write("claude/claude", script.as_bytes());
    set_mode(&claude, 0o700);
    claude
}

/// Whether the fake `claude` ran: it kept its arguments.
fn claude_ran(scratch: &Scratch) -> bool {
    scratch.path().join("claude/args").exists()
}

/// A configuration whose `[dictation] discovery` is `discovery` and whose `claude` is the
/// fake `claude`, run by `/bin/sh`, with `more` after it.
fn write_config(scratch: &Scratch, discovery: &Path, claude: &Path, more: &str) -> PathBuf {
    let text = format!(
        "[lsp]\nstart = \"manual\"\n\n[terminal]\nshell = \"/bin/sh\"\nlogin = false\n\n\
         [sessions]\nclaude = \"{}\"\n\n{more}\n[dictation]\ndiscovery = \"{}\"\n",
        claude.display(),
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

/// [`dictate`], where the selected window has no modeline of its own, as a docked composer:
/// the window it is docked under says `● REC`, and the echo area says how to stop.
fn dictate_docked(app: &mut App) {
    app.send_keys("s-d");
    echo_has(app, "the dictation recording", "Dictating: s-d stops");
    app.send_keys("s-d");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_dictation_types_into_the_buffer_where_it_started_and_undoes_in_one_step() {
    let scratch = Scratch::new("dictation-buffer");
    let fake = Fake::start(&scratch, &script(2));
    let claude = fake_claude(&scratch, 0, &no_fixes());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    dictate(&mut app);
    let state = app.wait_for("the text", |state| text(state).contains(SAID));
    assert!(
        text(&state).starts_with(&format!("{SAID}start")),
        "{}",
        text(&state)
    );
    // A fix's report, withheld, leaves the modeline out of the state while it shows.
    let modeline = state["modeline"].as_str().unwrap_or_default();
    assert!(!modeline.contains("REC"));
    assert!(!modeline.contains('…'));
    app.send_keys("C-z");
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
    let claude = fake_claude(&scratch, 0, &no_fixes());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
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
    let config = write_config(&scratch, &fake.discovery, &claude, "");
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
    dictate_docked(&mut app);
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
    let claude = fake_claude(&scratch, 0, &no_fixes());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
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
    // The person's M-v moved point: the yank goes at the end, after the `a`.
    app.send_keys("M-> C-y");
    app.wait_for("the yank", |state| {
        text(state).ends_with(&format!("a{SAID}"))
    });
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn no_discovery_file_says_typewhisper_is_not_running() {
    let scratch = Scratch::new("dictation-missing");
    let claude = fake_claude(&scratch, 0, &no_fixes());
    let config = write_config(
        &scratch,
        &scratch.path().join("none/api-discovery.json"),
        &claude,
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
    let claude = fake_claude(&scratch, 0, &no_fixes());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
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
    let claude = fake_claude(&scratch, 0, &no_fixes());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
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
    let claude = fake_claude(&scratch, 0, &no_fixes());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.send_keys("s-d");
    modeline_has(&mut app, "the dictation recording", "● REC");
    let state = echo_has(&mut app, "the end", "TypeWhisper stopped answering");
    assert!(!state["modeline"].as_str().unwrap().contains("REC"));
    assert!(!text(&state).contains(SAID));
}

/// [`script`], with a dictionary of one term and one correction.
fn script_with_dictionary() -> Value {
    let mut script = script(0);
    script["routes"]["GET /v1/dictionary/terms"] =
        json!([{"status": 200, "body": {"terms": ["oso"]}}]);
    script["routes"]["GET /v1/dictionary/corrections"] = json!([{"status": 200, "body": {
        "corrections": [{"original": "ty", "replacement": "TypeWhisper", "caseSensitive": false}]
    }}]);
    script
}

/// An answer that fixes `TypeWhisper` into `TW`, sure of it.
fn one_fix() -> Value {
    json!({"type": "result", "is_error": false, "structured_output": {"fixes": [
        {"wrong": "TypeWhisper", "right": "TW", "confidence": "high"},
    ]}})
}

fn fixes(state: &Value) -> u64 {
    state["dictation"]["fixes"].as_u64().unwrap()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_dictation_is_fixed_by_claude_and_one_undo_brings_back_what_was_said() {
    let scratch = Scratch::new("dictation-fix");
    let fake = Fake::start(&scratch, &script_with_dictionary());
    let claude = fake_claude(&scratch, 2, &one_fix());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    dictate(&mut app);
    let state = app.wait_for("the text, provisional", |state| {
        text(state).starts_with(SAID) && fixes(state) == 1
    });
    assert!(state["modeline"].as_str().unwrap().contains("fix  "));
    let state = app.wait_for("the fix landed", |state| fixes(state) == 0);
    assert!(
        text(&state).starts_with("hello from TWstart"),
        "{}",
        text(&state)
    );
    // While the report shows, withheld, the state leaves the modeline out too.
    let modeline = state["modeline"].as_str().unwrap_or_default();
    assert!(!modeline.contains("fix  "));
    // The report quotes the person's words and Claude's: the echo area alone shows it, withheld
    // from the dev socket as from agents.
    assert!(
        !state.to_string().contains("TypeWhisper → TW"),
        "the report is withheld"
    );
    app.send_keys("C-z");
    app.wait_for("one undo, back to what was said", |state| {
        text(state).starts_with(&format!("{SAID}start"))
    });
    app.send_keys("C-z");
    app.wait_for("another, back to the start", |state| {
        text(state).starts_with("start")
    });

    let args = fs::read_to_string(scratch.path().join("claude/args")).unwrap();
    for flag in [
        "--print",
        "--safe-mode",
        "--tools=",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--output-format=json",
        "--model=sonnet",
        "--effort=low",
    ] {
        assert!(args.lines().any(|line| line == flag), "{flag} in {args}");
    }
    assert!(!args.contains(SAID), "the text is never an argument");
    let input: Value =
        serde_json::from_slice(&fs::read(scratch.path().join("claude/input")).unwrap()).unwrap();
    assert_eq!(input["target"], SAID);
    assert_eq!(input["after"], "start\n");
    assert_eq!(input["before"], "");
    assert_eq!(input["terms"], json!(["oso"]));
    assert_eq!(
        input["corrections"],
        json!([{"original": "ty", "replacement": "TypeWhisper"}])
    );
    assert!(input["mode"].is_string());
    assert!(
        !input.to_string().contains("notes.md"),
        "the mode's name, never the path"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_provisional_face_is_drawn_only_while_the_fix_runs() {
    let scratch = Scratch::new("dictation-face");
    let fake = Fake::start(&scratch, &script(0));
    let claude = fake_claude(&scratch, 3, &no_fixes());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.visual_aids_off();
    dictate(&mut app);
    app.wait_for("the text, provisional", |state| {
        text(state).starts_with(SAID) && fixes(state) == 1
    });
    // Only macOS draws so far (ADR 0036): elsewhere the state alone is checked.
    let pixels = cfg!(target_os = "macos");
    let provisional = pixels.then(|| app.screenshot());
    let state = app.wait_for("the fix ended", |state| fixes(state) == 0);
    assert!(
        text(&state).starts_with(&format!("{SAID}start")),
        "no change"
    );
    if let Some(provisional) = provisional {
        // The text's rows, above the modeline and the echo area, which say what changed.
        let rows = 0..provisional.height / 2;
        let plain = app.screenshot();
        assert!(
            provisional.differs_in(&plain, rows) > 0,
            "the face is drawn while the fix runs, and goes with it"
        );
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn c_u_s_d_types_plain_and_never_runs_claude() {
    let scratch = Scratch::new("dictation-plain");
    let fake = Fake::start(&scratch, &script(0));
    let claude = fake_claude(&scratch, 0, &one_fix());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    app.send_keys("C-u s-d");
    modeline_has(&mut app, "the dictation recording", "● REC");
    app.send_keys("s-d");
    let state = app.wait_for("the text", |state| text(state).contains(SAID));
    assert_eq!(fixes(&state), 0);
    // Long enough for a fix to have started and run.
    thread::sleep(Duration::from_secs(2));
    let state = app.state();
    assert!(text(&state).starts_with(&format!("{SAID}start")));
    assert!(!state["modeline"].as_str().unwrap().contains("fix  "));
    assert!(!claude_ran(&scratch), "no fix, no claude");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_buffer_that_may_hold_secrets_never_runs_claude() {
    let scratch = Scratch::new("dictation-secret");
    let fake = Fake::start(&scratch, &script(0));
    let claude = fake_claude(&scratch, 0, &one_fix());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write(".env", b"KEY=\n");
    let mut app = start(&scratch, &config, &file);
    // The state says nothing of a secret buffer, its modeline included: the fake's requests
    // show the dictation's steps.
    let asked = |what: &str| {
        fake.requests()
            .iter()
            .any(|(request, ..)| request.starts_with(what))
    };
    app.send_keys("s-d");
    app.wait_for("the dictation started", |_| {
        asked("POST /v1/dictation/start")
    });
    // A press before it records stops it as it starts.
    app.send_keys("s-d");
    app.wait_for("its text asked for once stopped", |_| {
        asked("POST /v1/dictation/stop") && asked("GET /v1/dictation/transcription")
    });
    // Long enough for a fix to have started and run.
    thread::sleep(Duration::from_secs(3));
    let state = app.state();
    assert_eq!(fixes(&state), 0);
    assert!(
        !claude_ran(&scratch),
        "nothing of a secret buffer goes to claude"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_slow_fix_is_killed_at_its_limit_and_the_text_stays() {
    let scratch = Scratch::new("dictation-slow");
    let fake = Fake::start(&scratch, &script(0));
    let claude = fake_claude(&scratch, 300, &one_fix());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    dictate(&mut app);
    app.wait_for("the fix running", |state| fixes(state) == 1);
    let sleep = scratch.path().join("claude/sleep");
    app.wait_for("the fake's sleep", |_| sleep.exists());
    let pid: u32 = fs::read_to_string(&sleep).unwrap().trim().parse().unwrap();
    assert!(super::harness::process_exists(pid));
    // The limit is 30 s, as long as a wait: most of it passes here.
    thread::sleep(Duration::from_secs(25));
    let state = echo_has(&mut app, "the limit", "the fix failed: no answer in 30 s");
    assert_eq!(fixes(&state), 0);
    assert!(
        text(&state).starts_with(&format!("{SAID}start")),
        "kept as typed"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while super::harness::process_exists(pid) {
        assert!(
            Instant::now() < deadline,
            "what claude started is killed with it"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_fix_of_text_edited_meanwhile_is_skipped() {
    let scratch = Scratch::new("dictation-edited");
    let fake = Fake::start(&scratch, &script(0));
    let claude = fake_claude(&scratch, 3, &one_fix());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    dictate(&mut app);
    app.wait_for("the text, provisional", |state| {
        text(state).starts_with(SAID) && fixes(state) == 1
    });
    // Point is after the text: one back is inside it.
    app.send_keys("C-b x");
    app.wait_for("the edit", |state| {
        text(state).starts_with("hello from TypeWhispexr")
    });
    let state = echo_has(
        &mut app,
        "the skip",
        "Dictation fix skipped: its text was edited",
    );
    assert!(text(&state).starts_with("hello from TypeWhispexrstart"));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_fix_that_answers_during_the_next_dictation_waits_for_its_text() {
    let scratch = Scratch::new("dictation-two");
    // The first dictation's text at its first look once stopped; the second's, which starts
    // with the fake stopped already, after about 3 s of looks that say it is at work.
    let completed = json!({"status": 200, "body": {
        "id": ID,
        "status": "completed",
        "transcription": {"text": SAID, "raw_text": SAID},
    }});
    let processing = json!({"status": 200, "body": {"id": ID, "status": "processing"}});
    let mut transcription = vec![completed.clone()];
    transcription.extend(std::iter::repeat_n(processing, 10));
    transcription.push(completed);
    let mut two = script(0);
    two["routes"]["GET /v1/dictation/transcription"] = Value::Array(transcription);
    let fake = Fake::start(&scratch, &two);
    let claude = fake_claude(&scratch, 1, &one_fix());
    let config = write_config(&scratch, &fake.discovery, &claude, "");
    let file = scratch.write("notes.md", b"start\n");
    let mut app = start(&scratch, &config, &file);
    dictate(&mut app);
    app.wait_for("the first text, provisional", |state| {
        text(state).starts_with(SAID) && fixes(state) == 1
    });
    // The second, at once: the first fix answers while its text is due.
    app.send_keys("s-d");
    modeline_has(&mut app, "the second dictation waiting", "…");
    let state = app.wait_for("both texts, each fixed", |state| {
        fixes(state) == 0 && text(state).starts_with("hello from TWhello from TWstart")
    });
    assert!(!state.to_string().contains("kill ring"), "{state}");
}

/// A fake `claude` for a native session and a fix at once, in `scratch`'s `both` directory:
/// run with a JSON schema, it is the fix, which waits 3 s and answers [`one_fix`]; otherwise
/// it is the session, which keeps every message the person sends it.
fn fake_claude_for_both(scratch: &Scratch) -> PathBuf {
    let dir = scratch.path().join("both");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("answer.json"), one_fix().to_string()).unwrap();
    let dir = dir.display();
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\n  *--json-schema=*)\n    cat > /dev/null\n    sleep 3\n    \
         cat '{dir}/answer.json'\n    ;;\n  *)\n    while IFS= read -r line; do\n      \
         case \"$line\" in *'\"control_request\"'*) ;; *) printf '%s\\n' \"$line\" >> '{dir}/sent' ;; \
         esac\n    done\n    ;;\nesac\n"
    );
    let claude = scratch.write("both/claude", script.as_bytes());
    set_mode(&claude, 0o700);
    claude
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_composers_send_during_a_fix_waits_for_it_and_sends_the_fixed_text() {
    let scratch = Scratch::new("dictation-composer-send");
    let fake = Fake::start(&scratch, &script(0));
    let claude = fake_claude_for_both(&scratch);
    let config = write_config(&scratch, &fake.discovery, &claude, "");
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
    dictate_docked(&mut app);
    app.wait_for("the text in the composer, provisional", |state| {
        in_composer(state) && text(state) == format!("ask: {SAID}") && fixes(state) == 1
    });
    // The person sends while the fix runs: it waits.
    app.send_keys("C-c C-c");
    let sent = scratch.path().join("both/sent");
    thread::sleep(Duration::from_secs(1));
    let early = fs::read_to_string(&sent).unwrap_or_default();
    assert!(
        !early.contains("hello"),
        "nothing went before the fix: {early}"
    );
    app.wait_for("the fix landed", |state| fixes(state) == 0);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let went = fs::read_to_string(&sent).unwrap_or_default();
        if went.contains("ask: hello from TW") {
            assert!(!went.contains("TypeWhisper"), "the fixed text went: {went}");
            break;
        }
        assert!(Instant::now() < deadline, "the send never went: {went}");
        thread::sleep(Duration::from_millis(100));
    }

    // Again, but `C-c C-k` drops the wait: nothing more goes. The fake, stopped already, gives
    // the next text at its first look.
    app.send_keys("s-d");
    app.wait_for("the next text, provisional", |state| {
        in_composer(state) && text(state) == SAID && fixes(state) == 1
    });
    app.send_keys("C-c C-c C-c C-k");
    app.wait_for("the second fix landed", |state| fixes(state) == 0);
    thread::sleep(Duration::from_secs(1));
    let went = fs::read_to_string(&sent).unwrap_or_default();
    assert_eq!(
        went.matches("hello from").count(),
        1,
        "the dropped send never went: {went}"
    );
    let state = app.state();
    assert_eq!(
        text(&state),
        "hello from TW",
        "it stays in the composer, fixed"
    );
}
