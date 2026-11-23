//! Starts that wait for the configuration's first look (ADR 0173): a session and a terminal
//! started before the configuration's thread has read the file run with `[sessions]` and
//! `[terminal]` as the file has them once it has, and with the built-in settings once the wait
//! runs out; and several released together ask about a shared directory one at a time. A dev
//! build's `OSO_DEV_CONFIG_DELAY_MS` holds the first look back.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test: the queue and
//! its deadline have their own tests.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::harness::{App, Scratch};

/// A stand-in `claude`: what it was given and the variable `[sessions] env` passes, then a
/// wait. `DIR` is the flow's scratch directory.
const FAKE: &str = r#"#!/bin/sh
{
  for a in "$@"; do printf 'arg %s\n' "$a"; done
  printf 'passed %s\n' "$OSO_E2E_PASSED"
} > 'DIR'/run.$$
printf 'fake claude\n'
while :; do sleep 1; done
"#;

/// A stand-in shell for `[terminal] shell`: says it ran, with its arguments, then is
/// `/bin/sh`.
const SHELL: &str = r#"#!/bin/sh
printf '%s\n' "$*" > 'DIR'/shell.$$
exec /bin/sh "$@"
"#;

/// What `*Messages*` says when the wait ran out.
const WAITED_OUT: &str = "The configuration was not read in 5 s: what waited for it started \
                          with the built-in settings";

/// A flow's files: the scratch directory and the project the instance starts in.
struct Flow {
    scratch: Scratch,
    project: PathBuf,
    home: PathBuf,
    state: PathBuf,
    config: PathBuf,
}

impl Flow {
    fn new(test: &str) -> Flow {
        let scratch = Scratch::new(test);
        let project = scratch.path().join("proj");
        let home = scratch.path().join("home");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&home).unwrap();
        let dir = scratch.path().display().to_string();
        let fake = executable(&scratch, "fake-claude", &FAKE.replace("DIR", &dir));
        let shell = executable(&scratch, "fake-shell", &SHELL.replace("DIR", &dir));
        let config = format!(
            "[terminal]\nshell = \"{}\"\nlogin = false\n\n\
             [sessions]\nclaude = \"{}\"\nenv = [\"OSO_E2E_PASSED\"]\n",
            shell.display(),
            fake.display()
        );
        let config = scratch.write("config.toml", config.as_bytes());
        let state = scratch.path().join("state");
        Flow {
            scratch,
            project,
            home,
            state,
            config,
        }
    }

    /// An instance whose first look at the configuration comes `delay_ms` after it starts.
    fn start(&self, delay_ms: &str) -> App {
        let env: [(&str, &OsStr); 4] = [
            ("HOME", self.home.as_os_str()),
            ("OSO_STATE_DIR", self.state.as_os_str()),
            ("OSO_DEV_CONFIG_DELAY_MS", OsStr::new(delay_ms)),
            ("OSO_E2E_PASSED", OsStr::new("yes")),
        ];
        let mut app = App::start_with_config_and_env(&self.project, &self.config, &env);
        app.wait_until_ready();
        app
    }

    /// The files of the stand-ins' runs whose names start with `prefix`.
    fn written(&self, prefix: &str) -> Vec<String> {
        fs::read_dir(self.scratch.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
            .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
            .collect()
    }

    /// Waits for a file of a stand-in's run starting with `prefix` that `done` takes.
    fn wait_for(&self, prefix: &str, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(found) = self.written(prefix).into_iter().find(|text| done(text)) {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "no {prefix} run: {:?}",
                self.written(prefix)
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Writes `text` as an executable script called `name`.
fn executable(scratch: &Scratch, name: &str, text: &str) -> PathBuf {
    let path = scratch.write(name, text.as_bytes());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

/// The rows of `*Messages*`, shown for a moment.
fn messages(app: &mut App) -> Vec<Value> {
    app.send_keys("C-x b * M e s s a g e s * RET");
    let rows = app.call("get-text", Some(json!({ "start_row": 0, "rows": 100 })));
    app.send_keys("C-x b RET");
    rows["rows"].as_array().cloned().unwrap_or_default()
}

/// The terminal of the buffer named `name`.
fn terminal<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["terminals"]
        .as_array()?
        .iter()
        .find(|terminal| terminal["name"] == name)
}

/// Whether a stand-in shell wrote `path` for an interactive shell, the terminal's, which has
/// no arguments: a session's shell runs a command.
fn interactive(text: &str) -> bool {
    text.trim().is_empty()
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_session_and_a_terminal_started_before_the_first_look_run_as_configured() {
    let flow = Flow::new("starts-wait-look");
    let launched = Instant::now();
    let mut app = flow.start("4000");
    app.run_command("claude", None).unwrap();
    app.run_command("terminal", None).unwrap();
    // Nothing ran yet, while the look is surely still held back.
    if launched.elapsed() < Duration::from_secs(3) {
        assert!(
            flow.written("run.").is_empty(),
            "the session ran before the look"
        );
        assert!(
            flow.written("shell.").is_empty(),
            "a shell ran before the look"
        );
    }
    // Once looked: the configured `claude`, given the variable `[sessions] env` names.
    let run = flow.wait_for("run.", |_| true);
    assert!(run.lines().any(|line| line == "passed yes"), "{run}");
    assert!(run.lines().any(|line| line == "arg --name=proj"), "{run}");
    // And the configured shell, for the terminal.
    flow.wait_for("shell.", interactive);
    let state = app.wait_for("the session's terminal", |state| {
        terminal(state, "*claude: proj*").is_some()
    });
    assert!(terminal(&state, "*terminal*").is_some(), "{state}");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_look_that_never_comes_holds_a_start_five_seconds_at_most() {
    let flow = Flow::new("starts-wait-bound");
    let mut app = flow.start("30000");
    let asked = Instant::now();
    app.run_command("terminal", None).unwrap();
    // The built-in shell starts once the wait runs out, which `*Messages*` says.
    let deadline = Instant::now() + Duration::from_secs(15);
    while !messages(&mut app).contains(&json!(WAITED_OUT)) {
        assert!(
            Instant::now() < deadline,
            "*Messages* never said the wait ran out"
        );
        thread::sleep(Duration::from_millis(200));
    }
    assert!(
        asked.elapsed() >= Duration::from_secs(4),
        "the wait ran out early: {:?}",
        asked.elapsed()
    );
    app.wait_for("the built-in shell", |state| {
        terminal(state, "*terminal*").is_some_and(|terminal| terminal["running"] == true)
    });
    assert!(
        flow.written("shell.").is_empty(),
        "the configured shell ran before the look"
    );
}

/// The hosted sessions `state` lists.
fn sessions(state: &Value) -> usize {
    state["sessions"]["sessions"].as_array().map_or(0, Vec::len)
}

/// The open prompts' questions in `state`, the outermost first.
fn questions(state: &Value) -> Vec<String> {
    state["prompts"]
        .as_array()
        .map(|prompts| {
            prompts
                .iter()
                .map(|prompt| prompt["prompt"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `state` shows the shared-directory question alone.
fn asked_alone(state: &Value) -> bool {
    let asked = questions(state);
    asked.len() == 1 && asked[0].starts_with("Another session runs in this directory.")
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn starts_released_together_ask_about_a_shared_directory_one_at_a_time() {
    let flow = Flow::new("starts-wait-shared");
    let mut app = flow.start("3000");
    // Four presses before the look, all in the project: once it comes, the first starts and
    // each of the others would ask.
    for _ in 0..4 {
        app.run_command("claude", None).unwrap();
    }
    app.wait_for("the first session and one question", |state| {
        sessions(state) == 1 && asked_alone(state)
    });
    // The others wait their turn: nothing is asked over the second.
    thread::sleep(Duration::from_millis(500));
    let state = app.state();
    assert!(asked_alone(&state), "{:?}", questions(&state));
    // `y` starts the second, and then the third is asked.
    app.send_keys("y");
    app.wait_for("the second session and the third's question", |state| {
        sessions(state) == 2 && asked_alone(state)
    });
    // `n` drops it and the fourth, which waits for the same directory: nothing more is asked
    // or started.
    app.send_keys("n");
    app.wait_for("no question", |state| questions(state).is_empty());
    thread::sleep(Duration::from_millis(500));
    let state = app.state();
    assert_eq!(sessions(&state), 2, "{state}");
    assert!(questions(&state).is_empty(), "{:?}", questions(&state));
    let deadline = Instant::now() + Duration::from_secs(20);
    while flow.written("run.").len() < 2 {
        assert!(Instant::now() < deadline, "{:?}", flow.written("run."));
        thread::sleep(Duration::from_millis(100));
    }
}
