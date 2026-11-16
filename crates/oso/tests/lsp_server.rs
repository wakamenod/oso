//! A language server's life against the fake server (ADR 0138, step 3): `oso-lsp`'s session
//! thread starts it in its own group, initializes it, answers its requests, reads its
//! messages and standard error, keeps to the bounds by stopping it as a crash, starts it again
//! after one, stops it for the session after three, and shuts it down or ends it at a quit.
//! The fake server is a binary of this crate's dev builds, so these tests are here.
//!
//! Must not depend on: the app, the editor or the configuration; these drive the client's
//! `Server` directly.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use std::{env, fs};

use oso_lsp::protocol::Severity;
use oso_lsp::server::{Gate, Report, Server, Start, State};
use oso_platform::process::Executable;
use serde_json::{Value, json};

/// What a test has seen of a server.
struct Seen {
    server: Server,
    woken: Receiver<()>,
    states: Vec<State>,
    log: Vec<String>,
    shown: Vec<(Severity, String)>,
    dir: PathBuf,
}

/// A fresh directory for one test, its root.
fn scratch(name: &str) -> PathBuf {
    let dir = fs::canonicalize(env::temp_dir())
        .unwrap()
        .join(format!("oso-lsp-server-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    dir
}

fn start_of(dir: &std::path::Path, script: &Value) -> Start {
    let path = dir.join("script.json");
    fs::write(&path, script.to_string()).unwrap();
    Start {
        name: "fake".into(),
        number: 1,
        args: vec![path.into_os_string()],
        root: dir.to_owned(),
        inherit: Vec::new(),
        section: Some("fake".into()),
        options: Some(json!({"answer": 42, "inner": {"x": "y"}})),
    }
}

/// A script whose `initialize` is answered, with `rules` after it and `start` first.
fn script(start: Value, rules: Value) -> Value {
    let mut all = vec![json!({"method": "initialize", "do": [
        {"reply": {"capabilities": {"positionEncoding": "utf-8"}, "serverInfo": {"name": "fake"}}}
    ]})];
    all.extend(rules.as_array().cloned().unwrap_or_default());
    json!({"start": start, "rules": all, "log_answers": true})
}

fn started(name: &str, script: &Value, gate: Gate) -> Seen {
    let dir = scratch(name);
    let (wake, woken) = mpsc::channel();
    let server = Server::start(
        start_of(&dir, script),
        gate,
        Box::new(move || {
            let _ = wake.send(());
        }),
    )
    .unwrap();
    Seen {
        server,
        woken,
        states: Vec::new(),
        log: Vec::new(),
        shown: Vec::new(),
        dir,
    }
}

fn open() -> Gate {
    Box::new(|| Ok(fake()))
}

/// The fake server's program.
fn fake() -> Executable {
    Executable::absolute(env!("CARGO_BIN_EXE_fake-lsp-server")).unwrap()
}

impl Seen {
    fn take(&mut self, report: Report) {
        if self.states.last() != Some(&report.state) {
            self.states.push(report.state);
        }
        self.log.extend(report.log);
        self.shown
            .extend(report.shown.into_iter().map(|s| (s.severity, s.text)));
    }

    /// Waits up to `limit` for `done`, taking each report.
    fn until(&mut self, limit: Duration, done: impl Fn(&Seen) -> bool) {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(report) = self.server.take_report() {
                self.take(report);
            }
            if done(self) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out; states {:?}\nlog:\n{}\nshown: {:?}",
                self.states,
                self.log.join("\n"),
                self.shown
            );
            let _ = self
                .woken
                .recv_timeout(left.min(Duration::from_millis(100)));
        }
    }

    fn state(&self) -> Option<State> {
        self.states.last().copied()
    }

    fn logged(&self, part: &str) -> bool {
        self.log.iter().any(|line| line.contains(part))
    }

    /// The process number the log gave as it started.
    #[cfg(unix)]
    fn pid(&self) -> u32 {
        let line = self
            .log
            .iter()
            .find(|line| line.starts_with("started "))
            .expect("a start in the log");
        let after = line.split("(process ").nth(1).unwrap();
        after.split(')').next().unwrap().parse().unwrap()
    }
}

impl Drop for Seen {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const LIMIT: Duration = Duration::from_secs(20);

#[test]
fn a_server_starts_answers_and_shuts_down() {
    let mut seen = started(
        "life",
        &script(
            json!([]),
            json!([{"method": "initialized", "do": [
                {"request": {"method": "workspace/configuration", "params": {"items": [
                    {"section": "fake.inner"}, {"section": "other"}
                ]}}},
                {"request": {"method": "workspace/applyEdit", "params": {}}},
                {"request": {"method": "window/unknown", "params": {}}},
                {"notify": {"method": "window/logMessage", "params": {"type": 3, "message": "hello"}}},
                {"notify": {"method": "window/showMessage", "params": {"type": 2, "message": "careful"}}},
                {"stderr": {"text": "a line\nanother\n"}},
            ]}]),
        ),
        open(),
    );
    seen.until(LIMIT, |seen| {
        seen.state() == Some(State::Ready) && seen.logged("stderr: another")
    });
    assert!(seen.logged("initialized as fake in"), "{:?}", seen.log);
    assert!(seen.logged("positions in UTF-8"));
    assert!(seen.logged("log: hello"));
    assert!(seen.logged("stderr: a line"));
    seen.until(LIMIT, |seen| seen.logged(r#""error""#));
    let answers: Vec<Value> = seen
        .log
        .iter()
        .filter_map(|line| line.strip_prefix("stderr: answer: "))
        .map(|answer| serde_json::from_str(answer).unwrap())
        .collect();
    assert_eq!(answers[0]["result"], json!([{"x": "y"}, null]));
    assert_eq!(answers[1]["result"], json!({"applied": false}));
    assert_eq!(answers[2]["error"]["code"], json!(-32601));
    assert_eq!(seen.shown, [(Severity::Warning, "careful".to_owned())]);
    seen.server.shut_down();
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ended));
    // How the system writes a clean end: `exit status: 0`, or `exit code: 0` on Windows.
    let clean = if cfg!(windows) {
        "ended: exit code: 0"
    } else {
        "ended: exit status: 0"
    };
    assert!(seen.logged(clean), "{:?}", seen.log);
}

#[test]
fn a_crash_starts_it_again_and_three_stop_it() {
    let mut seen = started(
        "crashes",
        &script(
            json!([]),
            json!([{"method": "initialized", "do": [{"exit": 3}]}]),
        ),
        open(),
    );
    seen.until(Duration::from_secs(30), |seen| {
        seen.state() == Some(State::Stopped)
    });
    assert_eq!(
        seen.log
            .iter()
            .filter(|line| line.starts_with("crashed: "))
            .count(),
        3
    );
    assert!(seen.states.contains(&State::Waiting));
    assert!(seen.logged("starting again in 1 s"));
    assert!(seen.logged("starting again in 5 s"));
    let (severity, text) = seen.shown.last().unwrap();
    assert_eq!(*severity, Severity::Error);
    assert!(text.starts_with("stopped after crashing 3 times"), "{text}");
    // lsp-restart starts it again, its crashes forgotten.
    let restart = start_of(&seen.dir, &script(json!([]), json!([])));
    seen.server.restart(restart, open());
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ready));
}

/// A server that misbehaves as `start` and `rules` say crashes, and `*LSP*` says `why`.
fn crashes_for(name: &str, start: Value, rules: Value, why: &str) {
    let mut seen = started(name, &script(start, rules), open());
    seen.until(LIMIT, |seen| seen.logged("crashed: "));
    let line = seen
        .log
        .iter()
        .find(|line| line.starts_with("crashed: "))
        .unwrap();
    assert!(line.contains(why), "{line}; log:\n{}", seen.log.join("\n"));
    seen.server.shut_down();
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ended));
}

#[test]
fn a_broken_header_is_a_crash() {
    crashes_for(
        "header",
        json!([{"raw": "Content-Length: x\r\n\r\n"}]),
        json!([]),
        "sent a broken header",
    );
}

#[test]
fn a_message_that_is_not_json_rpc_is_a_crash() {
    crashes_for(
        "rpc",
        json!([{"raw": "Content-Length: 3\r\n\r\n[1]"}]),
        json!([]),
        "not JSON-RPC",
    );
    crashes_for(
        "json",
        json!([{"raw": "Content-Length: 3\r\n\r\n{{{"}]),
        json!([]),
        "not JSON",
    );
}

#[test]
fn an_encoding_oso_did_not_offer_is_a_crash() {
    let rules = json!([{"method": "initialize", "do": [
        {"reply": {"capabilities": {"positionEncoding": "utf-32"}}}
    ]}]);
    let mut seen = started("encoding", &json!({"rules": rules}), open());
    seen.until(LIMIT, |seen| seen.logged("crashed: "));
    assert!(seen.logged("chose a position encoding oso did not offer"));
    seen.server.shut_down();
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ended));
}

#[test]
fn a_refused_initialize_is_a_crash() {
    let rules = json!([{"method": "initialize", "do": [
        {"error": {"code": -32603, "message": "no workspace"}}
    ]}]);
    let mut seen = started("refused", &json!({"rules": rules}), open());
    seen.until(LIMIT, |seen| seen.logged("crashed: "));
    assert!(seen.logged("refused initialize: no workspace (-32603)"));
}

#[test]
fn a_flood_is_a_crash() {
    crashes_for(
        "flood",
        json!([]),
        json!([{"method": "initialized", "do": [{"flood": {"count": 10_001}}]}]),
        "sent more than 10000 messages",
    );
}

#[test]
fn an_oversized_message_is_dropped_and_the_server_goes_on() {
    let mut seen = started(
        "oversized",
        &script(
            json!([]),
            json!([{"method": "initialized", "do": [
                {"oversized": 16 * 1024 * 1024 + 1},
                {"notify": {"method": "window/logMessage", "params": {"type": 3, "message": "after"}}},
            ]}]),
        ),
        open(),
    );
    seen.until(LIMIT, |seen| seen.logged("log: after"));
    assert!(seen.logged("dropped a message of 16777217 bytes"));
    assert!(!seen.logged("crashed: "));
    assert_eq!(seen.state(), Some(State::Ready));
}

#[test]
fn progress_is_reported() {
    let begin = json!({"method": "$/progress", "params": {"token": "t", "value": {"kind": "begin", "title": "Indexing"}}});
    let report = json!({"method": "$/progress", "params": {"token": "t", "value": {"kind": "report", "percentage": 34}}});
    let mut seen = started(
        "progress",
        &script(
            json!([]),
            json!([
                {"method": "initialized", "do": [{"notify": begin}, {"notify": report}]},
                {"method": "shutdown", "do": [{"reply": null}]},
            ]),
        ),
        open(),
    );
    seen.until(LIMIT, |seen| seen.state() == Some(State::Working(Some(34))));
    seen.server.shut_down();
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ended));
}

#[test]
fn a_closed_gate_starts_nothing() {
    let mut seen = started(
        "gate",
        &script(json!([]), json!([])),
        Box::new(|| Err("the project is no longer trusted".into())),
    );
    seen.until(LIMIT, |seen| seen.state() == Some(State::Stopped));
    assert_eq!(
        seen.shown,
        [(
            Severity::Error,
            "not started: the project is no longer trusted".to_owned()
        )]
    );
    assert!(!seen.logged("started "));
}

#[test]
fn a_server_that_ignores_shutdown_is_ended_by_signals() {
    let mut seen = started(
        "stubborn",
        &script(
            json!([]),
            json!([
                {"method": "shutdown", "do": [{"hang": true}]},
                // It neither ends on `exit` nor reads that its input closed.
                {"method": "exit", "do": [{"sleep_ms": 60_000}]},
            ]),
        ),
        open(),
    );
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ready));
    let asked = Instant::now();
    seen.server.shut_down();
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ended));
    // shutdown's 2 s, then exit's 2 s, then SIGTERM, which a fake server does not ignore.
    assert!(
        asked.elapsed() >= Duration::from_secs(4),
        "{:?}",
        asked.elapsed()
    );
    #[cfg(unix)]
    assert!(seen.logged("ended: signal: 15"), "{:?}", seen.log);
}

#[cfg(unix)]
#[test]
fn a_quit_ends_the_server_at_once() {
    let mut seen = started("quit", &script(json!([]), json!([])), open());
    seen.until(LIMIT, |seen| seen.state() == Some(State::Ready));
    let pid = seen.pid();
    seen.server.quit();
    let deadline = Instant::now() + LIMIT;
    // The process ends; no one reaps it but this test's process, as oso would by exiting.
    while Instant::now() < deadline {
        if !runs(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the server still runs after a quit");
}

/// Whether process `pid` runs, as `ps` says: one gone, or a zombie, does not.
#[cfg(unix)]
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

/// A server whose group holds a process that ignores `SIGTERM`: a crash still ends it, as the
/// group gets `SIGKILL` before the server is reaped.
#[cfg(unix)]
#[test]
fn a_crash_ends_what_the_server_left_in_its_group() {
    let rules = json!([{"method": "initialized", "do": [{"exit": 3}]}]);
    let dir = scratch("survivor");
    let mut start = start_of(&dir, &script(json!([]), rules));
    let script_path = start.args[0].clone().into_string().unwrap();
    let shell = format!(
        "(trap '' TERM; exec sleep 60) & echo \"left $!\" >&2; exec '{}' '{}'",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
        script_path
    );
    start.args = vec!["-c".into(), shell.into()];
    let (wake, woken) = mpsc::channel();
    let server = Server::start(
        start,
        Box::new(|| Ok(Executable::absolute("/bin/sh").unwrap())),
        Box::new(move || {
            let _ = wake.send(());
        }),
    )
    .unwrap();
    let mut seen = Seen {
        server,
        woken,
        states: Vec::new(),
        log: Vec::new(),
        shown: Vec::new(),
        dir,
    };
    seen.until(LIMIT, |seen| {
        seen.logged("crashed: ") && seen.logged("ended: ")
    });
    let left: u32 = seen
        .log
        .iter()
        .find_map(|line| line.strip_prefix("stderr: left "))
        .expect("the shell said what it left")
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + LIMIT;
    while runs(left) {
        assert!(
            Instant::now() < deadline,
            "{left} still runs after the crash"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    seen.server.shut_down();
}
