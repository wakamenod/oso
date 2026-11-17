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
/// The rows of `*LSP*`, the servers' own: until a server's first row has made it, `C-x b`
/// makes an ordinary buffer of that name, which is killed, and asked for again. The state lists
/// no internal buffer, so the one shown is told by its being read-only as one.
fn lsp_log(app: &mut App, back: &str) -> String {
    let started = Instant::now();
    loop {
        app.send_keys(&format!("C-x b {} RET", super::typing("*LSP*")));
        if app.state()["buffer"]["read_only"] == "internal" {
            break;
        }
        app.send_keys("C-x k RET");
        app.send_keys(&format!("C-x b {} RET", super::typing(back)));
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no *LSP* of the servers' after 30 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 2000 })));
    app.send_keys(&format!("C-x b {} RET", super::typing(back)));
    text["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The rows of `*LSP*` once `count` of them hold `part`, which must exist. The modeline's
/// `lsp` reads the same while a server starts as when it is ready, so a flow waits on what it
/// reads.
fn lsp_log_with(app: &mut App, back: &str, part: &str, count: usize) -> String {
    let started = Instant::now();
    loop {
        let log = lsp_log(app, back);
        if log.matches(part).count() >= count {
            return log;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no {count} of {part:?} in *LSP*:\n{log}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
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
        own_session: false,
        set: &[],
        terminal: None,
        pass_fd: None,
        args_logged: true,
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
    let log = lsp_log_with(&mut app, "main.rs", "] initialized as fake in", 1);
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
            .is_some_and(|modeline| !modeline.contains("proj-lsp  lsp"))
    });
    until_gone(first);
    app.open_file(&root.join("src/lib.rs"));
    std::thread::sleep(Duration::from_millis(500));
    let modeline = app.state()["modeline"].as_str().unwrap().to_owned();
    assert!(!modeline.contains("proj-lsp  lsp"), "{modeline}");

    // lsp-start does.
    app.run_command("lsp-start", None).unwrap();
    modeline_has(&mut app, "the server is ready again", "proj-lsp  lsp  ");
    let second = server_pid(&lsp_log_with(
        &mut app,
        "lib.rs",
        "] initialized as fake in",
        2,
    ));
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
    assert!(!modeline.contains("proj-lsp  lsp"), "{modeline}");
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn quitting_oso_ends_its_servers() {
    let (_scratch, _root, mut app) = started("lsp-quit", &answering());
    // Its message comes after `initialize`, in a report that made `*LSP*`: `C-x b` before that
    // would make a buffer of that name of the person's.
    app.wait_for("its message", |state| {
        state["echo"].as_str() == Some("rust-analyzer: hello")
    });
    let pid = server_pid(&lsp_log_with(
        &mut app,
        "main.rs",
        "] initialized as fake in",
        1,
    ));
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
    let log = lsp_log_with(&mut app, "main.rs", "] crashed: ", 3);
    assert_eq!(log.matches("] crashed: ").count(), 3, "{log}");
    // lsp-restart tries again.
    app.run_command("lsp-restart", None).unwrap();
    modeline_has(&mut app, "it starts again", "proj-lsp  lsp  ");
}

/// FNV-1a, 64 bits, as the fake server hashes its copy of a document.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn listed(app: &mut App) {
    app.wait_for("a listing", |state| {
        state["prompts"][0]["completion"]["pending"] == false
    });
}

#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn edits_reach_the_servers_copy_and_a_write_or_a_kill_closes_it() {
    let rules = json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {"openClose": true, "change": 2, "save": true},
        }}}]},
        {"method": "textDocument/didOpen", "nth": 1, "do": [
            {"notify": {"method": "window/showMessage", "params": {"type": 3, "message": "opened"}}}
        ]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ]);
    let scratch = Scratch::new("lsp-sync");
    set_mode(scratch.path(), 0o700);
    let script = scratch.write(
        "conf/fake-lsp.json",
        json!({"rules": rules, "log_documents": true, "encoding": "utf-16"})
            .to_string()
            .as_bytes(),
    );
    let toml = format!(
        "[lsp.servers.rust-analyzer]\ncommand = [{:?}, {:?}]\n",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script.to_str().unwrap(),
    );
    let config = scratch.write("conf/config.toml", toml.as_bytes());
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    // The server says so once main.rs is open with it.
    app.wait_for("main.rs open with the server", |state| {
        state["echo"].as_str() == Some("rust-analyzer: opened")
    });

    // A key's edit reaches the server's copy.
    app.send_keys("é");
    let typed = format!("hash={:016x}", fnv("éfn main() {}\n".as_bytes()));
    lsp_log_with(&mut app, "main.rs", &typed, 1);

    // `C-x C-w` closes main.rs's document and opens renamed.rs's.
    app.send_keys("C-x C-w");
    listed(&mut app);
    app.send_keys(&super::typing("renamed.rs"));
    listed(&mut app);
    app.send_keys("RET");
    app.wait_for("the write", |state| {
        state["buffer"]["name"] == "renamed.rs" && state["buffer"]["modified"] == false
    });
    let log = lsp_log_with(&mut app, "renamed.rs", "/src/renamed.rs v", 1);
    assert!(
        log.lines()
            .any(|row| row.contains("stderr: closed") && row.ends_with("/src/main.rs")),
        "{log}"
    );

    // Killing the buffer closes its document.
    app.send_keys("C-x k RET");
    let back = app.state()["buffer"]["name"].as_str().unwrap().to_owned();
    let started = Instant::now();
    loop {
        let log = lsp_log(&mut app, &back);
        if log
            .lines()
            .any(|row| row.contains("stderr: closed") && row.ends_with("/src/renamed.rs"))
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no close of renamed.rs:\n{log}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The built-in table's other servers start as rust-analyzer does: a Go file in a trusted
/// project starts gopls, here the fake server named by `[lsp.servers.gopls] command`, which is
/// told gopls's settings, and the modeline says nothing of a server that is not installed.
#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_go_file_starts_gopls_with_its_settings_and_a_missing_server_shows_nothing() {
    let scratch = Scratch::new("lsp-gopls");
    set_mode(scratch.path(), 0o700);
    let rules = json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {}}}]},
        {"method": "initialized", "do": [
            {"request": {"method": "workspace/configuration", "params": {"items": [{"section": "gopls"}]}}}
        ]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ]);
    let script = scratch.write(
        "conf/fake-lsp.json",
        json!({"rules": rules, "log_answers": true})
            .to_string()
            .as_bytes(),
    );
    // gopls is the fake server; marksman, for the README, is a program that is not there.
    let toml = format!(
        "[lsp.servers.gopls]\ncommand = [{:?}, {:?}]\n[lsp.servers.marksman]\ncommand = [\"oso-no-such-marksman\"]\n",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script.to_str().unwrap(),
    );
    let config = scratch.write("conf/config.toml", toml.as_bytes());
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    scratch.write("proj-go/main.go", b"package main\n");
    scratch.write("proj-go/README.md", b"# go\n");
    scratch.write("proj-go/NOTES.md", b"# notes\n");
    let root = project(&scratch, "proj-go", &[]);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app =
        App::start_with_file_config_and_env(&root.join("main.go"), scratch.path(), &config, &[]);
    app.wait_until_ready();
    let state = modeline_has(&mut app, "gopls follows main.go", "proj-go  lsp  ");
    assert!(
        state["modeline"].as_str().unwrap().contains("Go"),
        "{}",
        state["modeline"]
    );
    let log = lsp_log_with(&mut app, "main.go", "stderr: answer: ", 1);
    assert!(log.contains("[gopls proj-go] initialized"), "{log}");
    assert!(log.contains(r#""usePlaceholders":true"#), "{log}");
    assert!(log.contains(r#""matcher":"CaseSensitive""#), "{log}");

    // A server that is not installed is said once, and the modeline says nothing of it.
    app.open_file(&root.join("README.md"));
    app.wait_for("marksman said not to be there", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("oso-no-such-marksman is not installed"))
    });
    let modeline = app.state()["modeline"].as_str().unwrap().to_owned();
    assert!(!modeline.contains("lsp"), "{modeline}");

    // Another Markdown file of the project: not looked for, nor said, again.
    app.open_file(&root.join("NOTES.md"));
    std::thread::sleep(Duration::from_millis(500));
    let log = lsp_log(&mut app, "NOTES.md");
    assert_eq!(
        log.matches("oso-no-such-marksman is not installed").count(),
        1,
        "{log}"
    );
}

/// Step 6 (ADR 0138, Diagnostics): a report for `main.rs`'s first version, which the fake server
/// sends as the file opens, shows its counts in the modeline; `C-M-n` and `C-M-p` move to each
/// diagnostic and wrap, and the echo area says the one at point once the cursor stays on it;
/// an edit before it keeps the move on it.
#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn diagnostics_are_counted_moved_to_and_said_at_point() {
    let scratch = Scratch::new("lsp-diagnostics");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    let uri = oso_lsp::uri::from_path(&root.join("src/main.rs")).unwrap();
    let range = |start: u32, end: u32| json!({"start": {"line": 0, "character": start}, "end": {"line": 0, "character": end}});
    // `fn main() {}`: a warning on `fn`, an error on `main`, and a hint, which is not counted.
    let rules = json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": true},
        }}}]},
        {"method": "textDocument/didOpen", "nth": 1, "do": [
            {"notify": {"method": "textDocument/publishDiagnostics", "params": {
                "uri": uri,
                "version": 1,
                "diagnostics": [
                    {"range": range(0, 2), "severity": 2, "message": "unused"},
                    {"range": range(3, 7), "severity": 1, "message": "mismatched types\nmore"},
                    {"range": range(8, 9), "severity": 4, "message": "a hint"},
                ],
            }}}
        ]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ]);
    let config = config(&scratch, &rules);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    modeline_has(&mut app, "the counts", "proj-lsp  lsp E1 W1  ");

    // `C-M-n` goes to `main`, and the echo area says its first line once the cursor stays.
    app.send_keys("C-M-n");
    app.wait_for("the message at point", |state| {
        state["echo"].as_str() == Some("rust-analyzer: mismatched types")
            && state["cursor"]["column"] == 3
    });
    // Past the last it wraps to the first, the hint skipped.
    app.send_keys("C-M-n");
    app.wait_for("the wrap", |state| {
        state["echo"].as_str() == Some("Wrapped") && state["cursor"]["column"] == 0
    });
    // Before the first it wraps to the last.
    app.send_keys("C-M-p");
    app.wait_for("the wrap back", |state| state["cursor"]["column"] == 3);

    // Text typed before `main` moves it, and `C-M-n` from the start still lands on it.
    app.send_keys("C-a x x");
    app.send_keys("C-a C-M-n C-M-n");
    app.wait_for("main, moved", |state| state["cursor"]["column"] == 5);
    let modeline = app.state()["modeline"].as_str().unwrap().to_owned();
    assert!(modeline.contains("lsp E1 W1  "), "{modeline}");
}

/// `C-c C-d` shows the server's description in a float at point, `C-v` scrolls it, and the next
/// key closes it and does what it always does.
#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_description_shows_at_point_scrolls_and_goes_with_the_next_key() {
    let scratch = Scratch::new("lsp-hover");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    let lines: String = (1..=40)
        .map(|n| format!("Line {n} of the docs.\n\n"))
        .collect();
    let rules = json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": true},
            "hoverProvider": true,
        }}}]},
        {"method": "textDocument/hover", "do": [{"reply": {"contents": {
            "kind": "markdown",
            "value": format!("```rust\nfn main()\n```\n\n{lines}"),
        }}}]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ]);
    let config = config(&scratch, &rules);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    modeline_has(&mut app, "the server is ready", "proj-lsp  lsp  ");
    // `lsp` reads the same while the server starts; it is ready once `*LSP*` says so.
    lsp_log_with(&mut app, "main.rs", "] initialized in ", 1);

    let before = app.screenshot();
    app.send_keys("C-c C-d");
    app.wait_for("the description", |state| state["hover"]["top"] == 0);
    let shown = app.screenshot();
    // The whole frame, as the float sits below the first line, past the topmost block of text
    // that `foreground` reads.
    let rows = 0..shown.height;
    assert!(
        shown.differs_in(&before, rows.clone()) > 0,
        "the float is drawn"
    );
    // `C-v` scrolls it, not the buffer. Only macOS draws text so far (ADR 0036): elsewhere
    // the float's rows look the same scrolled, and the state alone is checked.
    app.send_keys("C-v");
    app.wait_for("its next page", |state| state["hover"]["top"] == 18);
    if cfg!(target_os = "macos") {
        assert!(
            app.screenshot().differs_in(&shown, rows) > 0,
            "the float shows its next rows"
        );
    }
    // The next key closes it and moves the cursor as ever.
    app.send_keys("C-f");
    app.wait_for("it closed and the key ran", |state| {
        state["hover"].is_null() && state["cursor"]["column"] == 1
    });
}

/// `M-g d` lists the project's diagnostics, the current buffer's first, then a file no buffer
/// holds, whose row previews its place read from the file; `RET` visits it at that place.
#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn diagnostics_are_listed_previewed_and_gone_to() {
    let scratch = Scratch::new("lsp-diagnostics-list");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    scratch.write(
        "proj-lsp/src/lib.rs",
        b"// one\n// two\npub fn unused() {}\n",
    );
    let main = oso_lsp::uri::from_path(&root.join("src/main.rs")).unwrap();
    let lib = oso_lsp::uri::from_path(&root.join("src/lib.rs")).unwrap();
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    let rules = json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": true},
        }}}]},
        {"method": "textDocument/didOpen", "nth": 1, "do": [
            {"notify": {"method": "textDocument/publishDiagnostics", "params": {
                "uri": lib,
                "diagnostics": [
                    {"range": range(2, 7, 13), "severity": 2, "source": "rustc", "message": "unused   function"},
                ],
            }}},
            {"notify": {"method": "textDocument/publishDiagnostics", "params": {
                "uri": main,
                "version": 1,
                "diagnostics": [
                    {"range": range(0, 3, 7), "severity": 1, "message": "mismatched types"},
                ],
            }}},
        ]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ]);
    let config = config(&scratch, &rules);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    modeline_has(&mut app, "the counts", "proj-lsp  lsp E1  ");

    app.send_keys("M-g d");
    let state = app.wait_for("the rows", |state| {
        state["prompts"][0]["completion"]["count"] == 2
    });
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["prompt"], "Diagnostics in proj-lsp:");
    assert_eq!(
        prompt["completion"]["rows"],
        json!([
            "src/main.rs:1: error: mismatched types",
            "src/lib.rs:3: warning: unused function"
        ])
    );
    assert_eq!(
        prompt["completion"]["selected"], 0,
        "the first at or after point"
    );
    assert_eq!(prompt["completion"]["preview_kind"], "buffer");

    // The file no buffer holds is read for the preview, by its line.
    app.send_keys("C-n");
    app.wait_for("the file's excerpt", |state| {
        state["prompts"][0]["completion"]["preview_kind"] == "excerpt"
    });

    app.send_keys("RET");
    app.wait_for("the place in lib.rs", |state| {
        state["buffer"]["name"] == "lib.rs"
            && state["cursor"]["row"] == 2
            && state["cursor"]["column"] == 7
    });
}

/// An agent reads a buffer's diagnostics through the channel's `diagnostics`, hints left out,
/// and `state` gives the buffer's server and its counts (step 11).
#[cfg(unix)]
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_agent_reads_the_diagnostics_and_the_counts() {
    let scratch = Scratch::new("lsp-agent-diagnostics");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    let uri = oso_lsp::uri::from_path(&root.join("src/main.rs")).unwrap();
    let range = |start: u32, end: u32| json!({"start": {"line": 0, "character": start}, "end": {"line": 0, "character": end}});
    let rules = json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": true},
        }}}]},
        {"method": "textDocument/didOpen", "nth": 1, "do": [
            {"notify": {"method": "textDocument/publishDiagnostics", "params": {
                "uri": uri,
                "version": 1,
                "diagnostics": [
                    {"range": range(0, 2), "severity": 2, "message": "unused"},
                    {"range": range(3, 7), "severity": 1, "message": "mismatched\u{202e} types"},
                    {"range": range(8, 9), "severity": 4, "message": "a hint"},
                ],
            }}}
        ]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ]);
    let config = config(&scratch, &rules);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    modeline_has(&mut app, "the counts", "proj-lsp  lsp E1 W1  ");
    let mut agent = super::agent::let_in(&mut app);

    let answer = agent.call("diagnostics", None).unwrap();
    let entries = answer["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "no hint: {answer}");
    assert_eq!(
        (&entries[0]["severity"], &entries[0]["column"]),
        (&json!("warning"), &json!(1))
    );
    assert_eq!(
        entries[1]["message"], "mismatched\\u{202e} types",
        "escaped for the agent"
    );
    assert_eq!(entries[1]["server"], "rust-analyzer");
    assert_eq!(answer["more"], 0);

    let state = agent.call("state", None).unwrap();
    assert_eq!(state["version"], 4);
    let buffer = state["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|buffer| buffer["name"] == "main.rs")
        .unwrap();
    assert_eq!(buffer["server"]["name"], "rust-analyzer");
    assert_eq!(
        buffer["diagnostics"],
        json!({ "errors": 1, "warnings": 1, "information": 0 })
    );
}

/// A project whose `main.rs` the fake server follows, scripted with definitions and references
/// in it and in `lib/std.rs`, outside any project; the app on `main.rs`, its server following
/// it, and the file outside's URI.
fn answering_places(name: &str) -> (Scratch, App, String) {
    let scratch = Scratch::new(name);
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    // Outside any project: no `.git` above it in the scratch directory.
    let outside = scratch.write("lib/std.rs", b"// std\npub fn main() {}\n");
    let here = oso_lsp::uri::from_path(&root.join("src/main.rs")).unwrap();
    let there = oso_lsp::uri::from_path(&outside).unwrap();
    let range = |line: u32, character: u32| json!({"start": {"line": line, "character": character}, "end": {"line": line, "character": character + 4}});
    let script = json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": "utf-8",
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
            }}}]},
            {"method": "textDocument/definition", "nth": 1, "do": [
                {"reply": {"uri": here, "range": range(0, 3)}}
            ]},
            {"method": "textDocument/definition", "nth": 2, "do": [
                {"reply": [{"uri": there, "range": range(1, 7)}, {"uri": here, "range": range(0, 3)}]}
            ]},
            {"method": "textDocument/definition", "nth": 3, "do": [
                {"sleep_ms": 800},
                {"reply": {"uri": there, "range": range(1, 7)}}
            ]},
            {"method": "textDocument/references", "nth": 1, "do": [
                {"reply": [
                    {"uri": here, "range": range(0, 3)},
                    {"uri": there, "range": range(1, 7)},
                    {"uri": here, "range": range(0, 0)},
                ]}
            ]},
            {"method": "textDocument/references", "nth": 2, "do": [
                {"reply": [{"uri": here, "range": range(0, 3)}]}
            ]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "log_positions": true,
        "encoding": "utf-8",
    });
    let script = scratch.write("conf/fake-lsp.json", script.to_string().as_bytes());
    let toml = format!(
        "[lsp.servers.rust-analyzer]\ncommand = [{:?}, {:?}]\n",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script.to_str().expect("a UTF-8 scratch path"),
    );
    let config = scratch.write("conf/config.toml", toml.as_bytes());
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    lsp_log_with(&mut app, "main.rs", "stderr: doc ", 1);
    (scratch, app, there)
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_definition_is_gone_to_and_back_and_a_file_outside_the_project_joins() {
    let (_scratch, mut app, there) = answering_places("lsp-definition");

    // One answer: there, then back and forward again.
    app.send_keys("C-e M-.");
    app.wait_for("the definition", |state| state["cursor"]["column"] == 3);
    app.send_keys("M-,");
    app.wait_for("back", |state| state["cursor"]["column"] == 12);
    app.send_keys("C-M-,");
    app.wait_for("forward", |state| state["cursor"]["column"] == 3);

    // Two answers: listed, in the server's order, the file outside the project read for its
    // row and its preview; `RET` on it goes there, and it joins the server.
    app.send_keys("M-.");
    let state = app.wait_for("the list of definitions", |state| {
        state["prompts"][0]["completion"]["count"] == 2
    });
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["prompt"], "Definitions:");
    let rows = prompt["completion"]["rows"].clone();
    let first = rows[0].as_str().unwrap_or_default();
    assert!(first.ends_with("std.rs:2: pub fn main() {}"), "{rows}");
    assert_eq!(rows[1], "src/main.rs:1: fn main() {}", "{rows}");
    app.wait_for("the file's excerpt", |state| {
        state["prompts"][0]["completion"]["preview_kind"] == "excerpt"
    });
    app.send_keys("RET");
    app.wait_for("the file outside", |state| {
        state["buffer"]["name"] == "std.rs"
            && state["cursor"]["row"] == 1
            && state["cursor"]["column"] == 7
    });
    let log = lsp_log_with(&mut app, "std.rs", "stderr: doc ", 2);
    assert!(log.contains(&format!("stderr: doc {there} v1")), "{log}");
    assert!(log.contains("asked textDocument/definition 0:3"), "{log}");
    modeline_has(&mut app, "the server follows it", "lsp");

    // Back to main.rs; killing std.rs closes it with the server.
    app.send_keys("M-,");
    app.wait_for("back in main.rs", |state| {
        state["buffer"]["name"] == "main.rs"
    });
    app.send_keys(&format!("C-x k {} RET", super::typing("std.rs")));
    let log = lsp_log_with(&mut app, "main.rs", "stderr: closed ", 1);
    assert!(log.contains(&format!("stderr: closed {there}")), "{log}");

    // An answer that comes once the person moved on does not move them back.
    app.send_keys("C-a M-. C-e");
    lsp_log_with(&mut app, "main.rs", "answered with 1 place in", 2);
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "main.rs");
    assert_eq!(state["cursor"]["column"], 12);
}

/// `M-?` lists the references grouped by file, in the order the server first named each; `RET`
/// goes to the one picked in a file no buffer holds, `M-,` comes back, and one reference is gone
/// to at once.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn references_are_listed_by_file_and_one_is_gone_to_at_once() {
    let (_scratch, mut app, _) = answering_places("lsp-references");
    app.send_keys("C-e");
    app.wait_for("the line's end", |state| state["cursor"]["column"] == 12);

    app.send_keys("M-?");
    let state = app.wait_for("the list of references", |state| {
        state["prompts"][0]["completion"]["count"] == 3
    });
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["prompt"], "References:");
    let rows = prompt["completion"]["rows"].clone();
    assert_eq!(rows[0], "src/main.rs:1: fn main() {}", "{rows}");
    assert_eq!(rows[1], "src/main.rs:1: fn main() {}", "{rows}");
    let last = rows[2].as_str().unwrap_or_default();
    assert!(last.ends_with("std.rs:2: pub fn main() {}"), "{rows}");
    assert_eq!(prompt["completion"]["preview_kind"], "buffer");
    app.send_keys("C-n C-n RET");
    app.wait_for("the reference outside", |state| {
        state["buffer"]["name"] == "std.rs"
            && state["cursor"]["row"] == 1
            && state["cursor"]["column"] == 7
    });
    app.send_keys("M-,");
    app.wait_for("back in main.rs", |state| {
        state["buffer"]["name"] == "main.rs" && state["cursor"]["column"] == 12
    });

    // One reference is gone to at once.
    app.send_keys("M-?");
    app.wait_for("the one reference", |state| {
        state["cursor"]["column"] == 3 && state["echo"] == "1 reference"
    });
}

/// The agent reads the list of references the person has open as the person sees it: its rows,
/// the one selected and the preview's lines; its own `M-?` is still refused, and says where to
/// look (ADR 0138, step 15).
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_agent_reads_the_list_of_references_the_person_has_open() {
    let (_scratch, mut app, _) = answering_places("lsp-agent-references");
    let mut agent = super::agent::let_in(&mut app);
    app.send_keys("C-e");
    app.wait_for("the line's end", |state| state["cursor"]["column"] == 12);
    app.send_keys("M-? C-n");
    app.wait_for("the second reference selected", |state| {
        let completion = &state["prompts"][0]["completion"];
        completion["count"] == 3 && completion["selected"] == 1
    });
    let started = std::time::Instant::now();
    let list = loop {
        let state = agent.call("state", None).unwrap();
        let list = state["prompts"][0]["list"].clone();
        if list["selected"] == 1 && list["preview"]["kind"] == "buffer" {
            break list;
        }
        assert!(started.elapsed().as_secs() < 10, "no list: {state}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(list["of"], "references");
    assert_eq!(list["server"], "rust-analyzer");
    assert_eq!(list["count"], 3);
    let rows = &list["rows"];
    assert_eq!(
        rows[0],
        json!({ "path": "src/main.rs", "line": 1, "text": "fn main() {}" }),
        "{list}"
    );
    let outside = rows[2]["path"].as_str().unwrap_or_default();
    assert!(outside.ends_with("std.rs"), "{list}");
    assert_eq!(
        (&rows[2]["line"], &rows[2]["text"]),
        (&json!(2), &json!("pub fn main() {}"))
    );
    let preview = &list["preview"];
    let (first, at) = (preview["first_line"].as_u64(), preview["at"].as_u64());
    assert_eq!(
        first.zip(at).map(|(first, at)| first + at),
        Some(1),
        "{list}"
    );
    assert_eq!(preview["lines"][0], "fn main() {}", "{list}");

    app.send_keys("C-g");
    app.wait_for("the list closed", |state| state["prompts"] == json!([]));
    std::thread::sleep(std::time::Duration::from_millis(1300));
    let why = agent
        .run("lsp-find-references", None, "look")
        .expect_err("the agent's own M-? is refused");
    assert!(why.contains("`state`"), "{why}");
}

/// `C-M-i` asks for completions and opens the popup under the word, its items matched against
/// it in the server's order; typing narrows it, and `TAB` inserts the server's edit, replacing
/// the word.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn completion_at_point_pops_up_narrows_and_inserts() {
    let scratch = Scratch::new("lsp-completion");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    let script = json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": "utf-8",
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                "completionProvider": {},
            }}}]},
            {"method": "textDocument/completion", "do": [{"reply": {"isIncomplete": false, "items": [
                {"label": "format", "kind": 3, "detail": "macro format!", "textEdit": {"range": range(1, 0, 2), "newText": "format!()"}},
                {"label": "foo", "kind": 6},
                {"label": "bar", "kind": 6},
            ]}}]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "log_positions": true,
        "encoding": "utf-8",
    });
    let script = scratch.write("conf/fake-lsp.json", script.to_string().as_bytes());
    let toml = format!(
        "[lsp.servers.rust-analyzer]\ncommand = [{:?}, {:?}]\n",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script.to_str().expect("a UTF-8 scratch path"),
    );
    let config = scratch.write("conf/config.toml", toml.as_bytes());
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    lsp_log_with(&mut app, "main.rs", "stderr: doc ", 1);

    app.send_keys("M-> f o C-M-i");
    let state = app.wait_for("the popup", |state| !state["popup"].is_null());
    assert_eq!(state["popup"]["rows"], json!(["foo", "format"]), "{state}");
    assert_eq!(state["popup"]["selected"], 0);

    app.send_keys("r");
    app.wait_for("narrowed", |state| {
        state["popup"]["rows"] == json!(["format"])
    });
    app.send_keys("TAB");
    let state = app.wait_for("inserted", |state| state["popup"].is_null());
    assert_eq!(state["cursor"]["row"], 1);
    assert_eq!(state["cursor"]["column"], 9);
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 3 })));
    assert!(text.to_string().contains("format!()"), "{text}");
    // Last: showing the log closes a popup.
    // The fake server's stderr comes in its own time: waited for, as each line below.
    lsp_log_with(&mut app, "main.rs", "asked textDocument/completion 1:2", 1);
    lsp_log_with(&mut app, "main.rs", "answered with 3 completions", 1);
}

/// The person's typing asks by itself: a trigger character at once, three identifier
/// characters once the typing rests. The selected item is resolved, and `TAB` inserts it with
/// the `use` line its resolve gave, as one step.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn typing_opens_the_completion_popup_and_a_resolved_use_line_goes_in_with_it() {
    let scratch = Scratch::new("lsp-completion-typed");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    let at = |line: u32, character: u32| json!({"line": line, "character": character});
    let script = json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": "utf-8",
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                "completionProvider": {"triggerCharacters": ["."], "resolveProvider": true},
            }}}]},
            {"method": "textDocument/completion", "do": [{"reply": [
                {"label": "len", "kind": 2, "sortText": "1", "data": 1},
                {"label": "abcdef", "kind": 3, "sortText": "2", "data": 2},
            ]}]},
            {"method": "completionItem/resolve", "do": [{"reply": {
                "label": "len",
                "additionalTextEdits": [{"range": {"start": at(0, 0), "end": at(0, 0)}, "newText": "use std::fmt;\n"}],
            }}]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "encoding": "utf-8",
    });
    let script = scratch.write("conf/fake-lsp.json", script.to_string().as_bytes());
    let toml = format!(
        "[lsp.servers.rust-analyzer]\ncommand = [{:?}, {:?}]\n",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script.to_str().expect("a UTF-8 scratch path"),
    );
    let config = scratch.write("conf/config.toml", toml.as_bytes());
    set_mode(&scratch.path().join("conf"), 0o700);
    set_mode(&config, 0o644);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    lsp_log_with(&mut app, "main.rs", "stderr: doc ", 1);

    // A trigger character asks at once; the first row is resolved.
    app.send_keys("M-> x .");
    let state = app.wait_for("the popup", |state| !state["popup"].is_null());
    assert_eq!(state["popup"]["rows"], json!(["len", "abcdef"]), "{state}");
    // Its resolve's edit comes with it: wait for it to be asked and answered, then insert.
    std::thread::sleep(Duration::from_millis(400));
    app.send_keys("TAB");
    app.wait_for("inserted", |state| state["popup"].is_null());
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 4 })));
    let rows = text["rows"].clone();
    assert_eq!(rows[0], "use std::fmt;", "{text}");
    assert_eq!(rows[2], "x.len", "{text}");

    // Three identifier characters ask once the typing rests.
    app.send_keys("RET a b c");
    let state = app.wait_for("the popup after the rest", |state| {
        !state["popup"].is_null()
    });
    assert_eq!(state["popup"]["rows"], json!(["abcdef"]), "{state}");
    app.send_keys("C-g");
    app.wait_for("closed", |state| state["popup"].is_null());
    lsp_log_with(&mut app, "main.rs", "resolved with 1 more edits", 1);
}

/// The fake server's rules for code actions in the file at `uri`: one with its edit, one
/// disabled, and the preferred one, whose edit comes with its resolve.
fn code_action_rules(uri: &str) -> Value {
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": true},
            "codeActionProvider": {"resolveProvider": true},
        }}}]},
        {"method": "textDocument/codeAction", "do": [{"reply": [
            {"title": "Add a comment", "kind": "refactor", "edit": {"changes": {uri: [
                {"range": range(0, 0, 0), "newText": "// hi\n"},
            ]}}},
            {"title": "Inline it", "kind": "refactor.inline", "disabled": {"reason": "nothing to inline"}},
            {"title": "Rename to start", "kind": "quickfix", "isPreferred": true, "data": {"id": 1}},
        ]}]},
        {"method": "codeAction/resolve", "do": [{"reply": {
            "title": "Rename to start",
            "edit": {"changes": {uri: [{"range": range(0, 3, 7), "newText": "start"}]}},
        }}]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ])
}

/// `C-c C-r` lists the server's code actions, the preferred one selected and a disabled one
/// refused; `RET` resolves the picked action's edit and applies it, and another applies at once,
/// each one step to undo.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn code_actions_are_listed_picked_resolved_and_applied_one_step_each() {
    let scratch = Scratch::new("lsp-code-actions");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    let uri = oso_lsp::uri::from_path(&root.join("src/main.rs")).unwrap();
    let rules = code_action_rules(&uri);
    let config = config(&scratch, &rules);
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    lsp_log_with(&mut app, "main.rs", "] initialized in ", 1);
    let text = |app: &mut App| {
        app.call("get-text", Some(json!({ "start_row": 0, "rows": 3 })))
            .to_string()
    };

    app.send_keys("C-c C-r");
    let state = app.wait_for("the list of code actions", |state| {
        state["prompts"][0]["completion"]["count"] == 3
    });
    let prompt = &state["prompts"][0];
    assert_eq!(prompt["prompt"], "Code action:");
    assert_eq!(
        prompt["completion"]["rows"],
        json!(["Add a comment", "Inline it", "Rename to start"]),
        "{prompt}"
    );
    assert_eq!(prompt["completion"]["selected"], 2, "the preferred one");
    // A disabled one is refused, and the list stays.
    app.send_keys("C-p RET");
    app.wait_for("refused", |state| {
        state["prompts"][0]["message"] == "This code action is disabled: nothing to inline"
    });
    app.send_keys("C-n RET");
    // The echo first: showing `*LSP*` below clears it.
    app.wait_for("resolved and applied", |state| {
        state["prompts"].as_array().is_some_and(Vec::is_empty)
            && state["echo"]
                .as_str()
                .is_some_and(|echo| echo.contains("1 edit in 1 buffer"))
    });
    lsp_log_with(&mut app, "main.rs", "resolved a code action", 1);
    assert!(text(&mut app).contains("fn start()"), "{}", text(&mut app));

    // Another applies at once, its edit in the answer.
    app.send_keys("C-c C-r");
    app.wait_for("the list again", |state| {
        state["prompts"][0]["completion"]["count"] == 3
    });
    app.send_keys("C-p C-p RET");
    app.wait_for("commented", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("Add a comment"))
    });
    assert!(text(&mut app).contains("// hi"), "{}", text(&mut app));
    // One undo takes the whole action back, and only it.
    app.send_keys("C-z");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut after = text(&mut app);
    while after.contains("// hi") && std::time::Instant::now() < deadline {
        after = text(&mut app);
    }
    assert!(
        !after.contains("// hi") && after.contains("fn start()"),
        "{after}"
    );
}

/// The fake server's rules for 13b: an action whose edit changes `lib`, a file no buffer holds,
/// and one that runs a declared command, whose edit of `main` comes as `workspace/applyEdit`.
fn code_action_file_rules(main: &str, lib: &str) -> Value {
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    json!([
        {"method": "initialize", "do": [{"reply": {"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": true},
            "codeActionProvider": true,
            "executeCommandProvider": {"commands": ["fake.fix"]},
        }}}]},
        {"method": "textDocument/codeAction", "do": [{"reply": [
            {"title": "Edit the library", "edit": {"changes": {lib: [
                {"range": range(0, 0, 0), "newText": "// edited\n"},
            ]}}},
            {"title": "Fix by command", "command": {"title": "Fix", "command": "fake.fix"}},
        ]}]},
        {"method": "workspace/executeCommand", "do": [
            {"request": {"method": "workspace/applyEdit", "params": {"edit": {"changes": {main: [
                {"range": range(0, 3, 7), "newText": "begin"},
            ]}}}}},
            {"sleep_ms": 300},
            {"reply": null},
        ]},
        {"method": "shutdown", "do": [{"reply": null}]},
    ])
}

/// An action that changes a file no buffer holds asks first, and `y` opens it and applies the
/// edit; an action that runs a declared command applies the edit the server sends while it runs.
#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_code_action_opens_a_file_after_a_yes_and_a_command_s_edit_applies() {
    let scratch = Scratch::new("lsp-code-action-files");
    set_mode(scratch.path(), 0o700);
    let root = project(&scratch, "proj-lsp", &["main.rs"]);
    scratch.write("proj-lsp/src/lib.rs", b"pub fn lib() {}\n");
    let main = oso_lsp::uri::from_path(&root.join("src/main.rs")).unwrap();
    let lib = oso_lsp::uri::from_path(&root.join("src/lib.rs")).unwrap();
    let config = config(&scratch, &code_action_file_rules(&main, &lib));
    let record = scratch.write("conf/trusted-projects", record_line(&root).as_bytes());
    set_mode(&record, 0o600);
    let mut app = App::start_with_file_config_and_env(
        &root.join("src/main.rs"),
        scratch.path(),
        &config,
        &[],
    );
    app.wait_until_ready();
    lsp_log_with(&mut app, "main.rs", "] initialized in ", 1);

    app.send_keys("C-c C-r");
    app.wait_for("the list", |state| {
        state["prompts"][0]["completion"]["count"] == 2
    });
    app.send_keys("RET");
    let state = app.wait_for("the question", |state| {
        state["prompts"][0]["prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("1 not open"))
    });
    assert!(state["buffers"].to_string().contains("main.rs"), "{state}");
    app.send_keys("y");
    let state = app.wait_for("the library edited", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("1 edit in 1 buffer"))
    });
    let lib_buffer = state["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|buffer| buffer["name"] == "lib.rs")
        .cloned()
        .unwrap_or_else(|| panic!("lib.rs opened: {state}"));
    assert_eq!(lib_buffer["modified"], true, "{lib_buffer}");
    assert_eq!(state["buffer"]["name"], "main.rs", "it opened unshown");

    // A declared command's edit, as it runs.
    app.send_keys("C-c C-r");
    app.wait_for("the list again", |state| {
        state["prompts"][0]["completion"]["count"] == 2
    });
    app.send_keys("C-n RET");
    app.wait_for("the command's edit", |state| {
        state["echo"]
            .as_str()
            .is_some_and(|echo| echo.contains("1 edit in 1 buffer"))
            && state["buffer"]["modified"] == true
    });
    let text = app.call("get-text", Some(json!({ "start_row": 0, "rows": 2 })));
    assert!(text.to_string().contains("fn begin()"), "{text}");
}
