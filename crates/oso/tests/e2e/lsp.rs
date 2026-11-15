//! A language server's life in the running app (ADR 0138, step 3), against the fake server
//! that the configuration names as rust-analyzer's command: a visit starts it and the modeline
//! says `lsp`, `*LSP*` says what it did and the echo area names it before its message,
//! `lsp-shutdown` stops it and a visit does not start it again, `lsp-start` does, untrusting
//! the project stops it, a server that crashes starts again and stops after three crashes with
//! `lsp!`, and quitting oso ends it. The dev socket's keys and commands stand in for the
//! person's.
//!
//! Must not depend on: the app's internals, or what the client's own tests check.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::config::set_mode;
use super::harness::{App, Scratch};
use super::starts::{project, record_line};

/// A configuration whose rust-analyzer is the fake server with `rules`.
fn config(scratch: &Scratch, rules: &Value) -> PathBuf {
    let script = scratch.write(
        "conf/fake-lsp.json",
        json!({ "rules": rules }).to_string().as_bytes(),
    );
    let toml = format!(
        "[lsp.servers.rust-analyzer]\ncommand = [{:?}, {:?}]\n",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script.to_str().expect("a UTF-8 scratch path"),
    );
    let config = scratch.write("conf/config.toml", toml.as_bytes());
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    config
}

/// A trusted project with `main.rs`, and the app visiting it with `rules`.
fn started(name: &str, rules: &Value) -> (Scratch, PathBuf, App) {
    let scratch = Scratch::new(name);
    set_mode(scratch.path(), 0o700);
    let config = config(&scratch, rules);
    let root = project(&scratch, "proj-lsp", &["main.rs", "lib.rs"]);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    (scratch, root, app)
}

fn modeline_has(app: &mut App, what: &str, part: &'static str) -> Value {
    app.wait_for(what, move |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains(part))
    })
}

/// The rows of `*LSP*`, read in the selected window, which then shows `back` again.
fn lsp_log(app: &mut App, back: &str) -> String {
    app.send_keys("C-x b *LSP* RET");
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 2000 })));
    app.send_keys(&format!("C-x b {back} RET"));
    text["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The process the log says the server started as, the last one.
fn server_pid(log: &str) -> u32 {
    let line = log
        .lines()
        .rev()
        .find(|line| line.contains("] started "))
        .unwrap_or_else(|| panic!("no start in *LSP*:\n{log}"));
    let after = line.split("(process ").nth(1).unwrap();
    after.split(')').next().unwrap().parse().unwrap()
}

/// Whether process `pid` runs, as `ps` says: one gone, or a zombie, does not.
fn runs(pid: u32) -> bool {
    use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
    let ps = spawn(&Spawn {
        executable: &Executable::from_path("ps", None).unwrap(),
        args: &[
            "-o".into(),
            "stat=".into(),
            "-p".into(),
            pid.to_string().into(),
        ],
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Piped,
        stderr: Sink::Null,
        own_process_group: false,
        set: &[],
        terminal: None,
        pass_fd: None,
    })
    .unwrap();
    let out = ps.wait_with_output().unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    !stat.trim().is_empty() && !stat.trim().starts_with('Z')
}

fn until_gone(pid: u32) {
    let started = Instant::now();
    while runs(pid) {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the server {pid} still runs"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn answering() -> Value {
    json!([
        {"method": "initialize", "do": [
            {"reply": {"capabilities": {"positionEncoding": "utf-8"}, "serverInfo": {"name": "fake"}}}
        ]},
        {"method": "initialized", "do": [
            {"notify": {"method": "window/showMessage", "params": {"type": 3, "message": "hello"}}}
        ]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ])
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_server_starts_on_a_visit_stops_when_asked_and_ends_with_oso() {
    let (_scratch, root, mut app) = started("lsp-life", &answering());
    modeline_has(&mut app, "the server is ready", "proj-lsp  lsp  ");
    app.wait_for("its message", |state| {
        state["echo"].as_str() == Some("rust-analyzer: hello")
    });
    let log = lsp_log(&mut app, "main.rs");
    assert!(
        log.contains("[rust-analyzer proj-lsp] initialized as fake in"),
        "{log}"
    );
    let first = server_pid(&log);
    assert!(runs(first));

    // lsp-shutdown stops it, and a visit of another file does not start it again.
    app.run_command("lsp-shutdown", None).unwrap();
    app.wait_for("the server stopped", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.contains("lsp"))
    });
    until_gone(first);
    app.open_file(&root.join("src/lib.rs"));
    std::thread::sleep(Duration::from_millis(500));
    let modeline = app.state()["modeline"].as_str().unwrap().to_owned();
    assert!(!modeline.contains("lsp"), "{modeline}");

    // lsp-start does.
    app.run_command("lsp-start", None).unwrap();
    modeline_has(&mut app, "the server is ready again", "proj-lsp  lsp  ");
    let second = server_pid(&lsp_log(&mut app, "lib.rs"));
    assert_ne!(first, second);

    // Untrusting the project stops it.
    app.run_command("untrust-project", None).unwrap();
    modeline_has(
        &mut app,
        "the project is restricted",
        "proj-lsp  restricted  ",
    );
    until_gone(second);
    let modeline = app.state()["modeline"].as_str().unwrap().to_owned();
    assert!(!modeline.contains("lsp"), "{modeline}");
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn quitting_oso_ends_its_servers() {
    let (_scratch, _root, mut app) = started("lsp-quit", &answering());
    modeline_has(&mut app, "the server is ready", "proj-lsp  lsp  ");
    let pid = server_pid(&lsp_log(&mut app, "main.rs"));
    let _ = app.exchange("exit", None);
    app.wait_for_exit();
    until_gone(pid);
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_server_that_keeps_crashing_stops_after_three_crashes() {
    let crashing = json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {}}}]},
        {"method": "initialized", "do": [{"exit": 3}]},
    ]);
    let (_scratch, _root, mut app) = started("lsp-crash", &crashing);
    // A crash, a start again after 1 s, another after 5 s, and a third that stops it.
    app.wait_for("the server stopped for the session", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("proj-lsp  lsp!  "))
    });
    let echo = app.state()["echo"].as_str().unwrap_or_default().to_owned();
    assert!(
        echo.starts_with("rust-analyzer: stopped after crashing 3 times"),
        "{echo}"
    );
    let log = lsp_log(&mut app, "main.rs");
    assert_eq!(log.matches("] crashed: exited").count(), 3, "{log}");
    // lsp-restart tries again.
    app.run_command("lsp-restart", None).unwrap();
    modeline_has(&mut app, "it starts again", "proj-lsp  lsp  ");
}
