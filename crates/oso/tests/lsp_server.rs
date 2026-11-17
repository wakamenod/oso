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
use oso_lsp::server::{Found, Gate, Report, Server, Start, State};
use oso_platform::process::Executable;
use serde_json::{Value, json};

/// What a test has seen of a server.
struct Seen {
    server: Server,
    woken: Receiver<()>,
    states: Vec<State>,
    log: Vec<String>,
    shown: Vec<(Severity, String)>,
    documents: Vec<(u64, Option<oso_lsp::server::Closed>)>,
    diagnosed: Vec<oso_lsp::server::Diagnosed>,
    answers: Vec<oso_lsp::server::Answer>,
    completing: Option<oso_lsp::protocol::Completing>,
    apply_edits: Vec<oso_lsp::server::ApplyEdit>,
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
        documents: Vec::new(),
        diagnosed: Vec::new(),
        answers: Vec::new(),
        completing: None,
        apply_edits: Vec::new(),
        dir,
    }
}

fn open() -> Gate {
    // The script is the one `start_of` wrote last into the directory the server runs in.
    Box::new(|| {
        Ok(Found {
            executable: fake(),
            args: vec!["script.json".into()],
            notes: Vec::new(),
        })
    })
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
        self.documents.extend(report.documents);
        self.diagnosed.extend(report.diagnostics);
        self.answers.extend(report.answers);
        self.apply_edits.extend(report.apply_edits);
        if report.completing.is_some() {
            self.completing = report.completing;
        }
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
    // No command of its waits: its edit is refused, and says why (step 13b).
    assert_eq!(answers[1]["result"]["applied"], json!(false));
    assert!(answers[1]["result"]["failureReason"].is_string());
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
    seen.until(LIMIT, |seen| seen.state() == Some(State::NotStarted));
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
        own_session: false,
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
    let start = start_of(&dir, &script(json!([]), rules));
    let shell = format!(
        "(trap '' TERM; exec sleep 60) & echo \"left $!\" >&2; exec '{}' script.json",
        env!("CARGO_BIN_EXE_fake-lsp-server"),
    );
    let (wake, woken) = mpsc::channel();
    let server = Server::start(
        start,
        Box::new(move || {
            Ok(Found {
                executable: Executable::absolute("/bin/sh").unwrap(),
                args: vec!["-c".into(), shell.clone().into()],
                notes: Vec::new(),
            })
        }),
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
        documents: Vec::new(),
        diagnosed: Vec::new(),
        answers: Vec::new(),
        completing: None,
        apply_edits: Vec::new(),
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

/// FNV-1a, 64 bits, as the fake server hashes its copy.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A script that answers `initialize` with `encoding` and incremental sync, and keeps and logs
/// documents in that encoding.
fn syncing(encoding: &str) -> Value {
    json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": encoding,
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
            }}}]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "encoding": encoding,
    })
}

/// A small generator of its own, so that a failure can be run again from its seed.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// A random batch for `text`: ascending, apart splices on character starts, and the text after.
/// Its pieces hold multibyte characters, surrogate pairs and `\r\n`, never a lone `\r`.
fn random_batch(rng: &mut Lcg, text: &str) -> (Vec<oso_core::syntax::Splice>, String) {
    const PIECES: [&str; 8] = [
        "a",
        "é",
        "日",
        "\u{1F600}",
        "\n",
        "\r\n",
        " ",
        "fn x() {}\n",
    ];
    let mut splices = Vec::new();
    let mut after = String::new();
    let mut at = 0;
    for _ in 0..rng.below(4) + 1 {
        let mut start = text.floor_char_boundary((at + rng.below(40)).min(text.len()));
        // Never between a \r and its \n, which would leave a lone \r.
        if start > 0 && text.as_bytes()[start - 1] == b'\r' {
            start -= 1;
        }
        start = start.max(at);
        let mut end = text.floor_char_boundary((start + rng.below(12)).min(text.len()));
        if end > 0 && end < text.len() && text.as_bytes()[end - 1] == b'\r' {
            end += 1;
        }
        let insert: String = (0..rng.below(5))
            .map(|_| PIECES[rng.below(PIECES.len())])
            .collect();
        after.push_str(&text[at..start]);
        after.push_str(&insert);
        splices.push(oso_core::syntax::Splice {
            start,
            old_len: end - start,
            new_len: insert.len(),
        });
        at = end;
        if at >= text.len() {
            break;
        }
    }
    after.push_str(&text[at..]);
    (splices, after)
}

/// Random batches keep the fake server's copy equal to the text, in `encoding`.
fn the_server_s_copy_follows_random_batches(encoding: &str, seed: u64) {
    use oso_core::rope::Rope;
    use oso_lsp::server::{Change, Document};
    let mut seen = started(&format!("sync-{encoding}"), &syncing(encoding), open());
    let mut text = String::from("fn main() {\r\n    let é = \"日\u{1F600}\";\n}\n");
    seen.server.open(Document {
        id: 7,
        uri: "file:///w/p/main.rs".into(),
        language_id: "rust".into(),
        text: Rope::from(text.as_str()),
        revision: 0,
    });
    let mut expected = vec![(1, fnv(text.as_bytes()))];
    // Batches before the server is ready only change what it is opened with: one version.
    seen.until(LIMIT, |seen| seen.logged(" v1 "));
    let mut rng = Lcg(seed);
    for version in 2..=150 {
        let (splices, after) = random_batch(&mut rng, &text);
        seen.server.change(Change {
            id: 7,
            before: Rope::from(text.as_str()),
            after: Rope::from(after.as_str()),
            splices: Some(splices),
            revision: 0,
        });
        text = after;
        expected.push((version, fnv(text.as_bytes())));
    }
    seen.until(LIMIT, |seen| seen.logged(" v150 "));
    for (version, hash) in expected {
        let line = format!(" v{version} len=");
        let logged = seen
            .log
            .iter()
            .find(|row| row.contains(&line))
            .unwrap_or_else(|| panic!("no version {version} in the log"));
        assert!(
            logged.ends_with(&format!("hash={hash:016x}")),
            "version {version} (seed {seed}): {logged}"
        );
    }
    seen.server.shut_down();
}

#[test]
fn the_server_s_copy_follows_random_batches_in_utf_8() {
    the_server_s_copy_follows_random_batches("utf-8", 1);
}

#[test]
fn the_server_s_copy_follows_random_batches_in_utf_16() {
    the_server_s_copy_follows_random_batches("utf-16", 2);
}

#[test]
fn a_lone_carriage_return_closes_the_document() {
    use oso_core::rope::Rope;
    use oso_lsp::server::{Change, Closed, Document};
    let mut seen = started("lone-cr", &syncing("utf-16"), open());
    seen.server.open(Document {
        id: 3,
        uri: "file:///w/p/a.rs".into(),
        language_id: "rust".into(),
        text: Rope::from("a\r\nb"),
        revision: 0,
    });
    seen.until(LIMIT, |seen| seen.logged(" v1 "));
    // The \n of the \r\n deleted.
    seen.server.change(Change {
        id: 3,
        before: Rope::from("a\r\nb"),
        after: Rope::from("a\rb"),
        splices: Some(vec![oso_core::syntax::Splice {
            start: 2,
            old_len: 1,
            new_len: 0,
        }]),
        revision: 0,
    });
    seen.until(LIMIT, |seen| {
        seen.logged("stderr: closed file:///w/p/a.rs") && seen.documents.len() == 2
    });
    assert_eq!(
        seen.documents,
        [(3, None), (3, Some(Closed::LoneCarriageReturn))],
        "opened, then kept from the server"
    );
    assert!(seen.logged("a document is not sent: it holds a carriage return that ends no line"));
    seen.server.shut_down();
}

#[test]
fn batches_before_the_server_is_ready_go_with_its_open() {
    use oso_core::rope::Rope;
    use oso_lsp::server::{Change, Document};
    let mut seen = started("early", &syncing("utf-8"), open());
    seen.server.open(Document {
        id: 1,
        uri: "file:///w/p/b.rs".into(),
        language_id: "rust".into(),
        text: Rope::from("a"),
        revision: 0,
    });
    seen.server.change(Change {
        id: 1,
        before: Rope::from("a"),
        after: Rope::from("ab"),
        splices: None,
        revision: 0,
    });
    seen.until(LIMIT, |seen| seen.logged(" v1 "));
    let hash = format!("len=2 hash={:016x}", fnv(b"ab"));
    assert!(seen.logged(&hash), "{:?}", seen.log);
    seen.server.shut_down();
}

/// Random edits at several places at once, and undos, made by the editor on a buffer a server
/// follows: each batch the buffer's log gives keeps the fake server's copy equal to the text.
fn a_followed_buffers_batches_keep_the_copy(encoding: &str, seed: u64) {
    use oso_core::buffer::{Buffer, Replacement};
    use oso_core::editor::Editor;
    use oso_lsp::server::{Change, Document};
    let mut seen = started(&format!("buffer-{encoding}"), &syncing(encoding), open());
    let mut editor = Editor::with_buffer(Buffer::from_text(
        "t.rs",
        "fn main() {\r\n    let é = \"日\u{1F600}\";\n}\n",
    ));
    let id = editor.window().buffer();
    editor.buffer_mut(id).unwrap().set_followed(true);
    let text = |editor: &Editor| editor.buffer(id).unwrap().text().clone();
    seen.server.open(Document {
        id: 9,
        uri: "file:///w/p/t.rs".into(),
        language_id: "rust".into(),
        text: text(&editor),
        revision: 0,
    });
    seen.until(LIMIT, |seen| seen.logged(" v1 "));
    let mut sent = text(&editor);
    let mut revision = editor.buffer(id).unwrap().revision();
    let mut version = 1;
    let mut rng = Lcg(seed);
    let mut last_hash = 0;
    for _ in 0..120 {
        if rng.below(5) == 0 {
            let _ = editor.undo();
        } else {
            let now = sent.to_string();
            let (splices, _) = random_batch(&mut rng, &now);
            let inserts: Vec<String> = splices
                .iter()
                .map(|splice| {
                    ["x", "é", "\u{1F600}", "\n", "\r\n", ""][splice.new_len % 6].to_owned()
                })
                .collect();
            let edits: Vec<Replacement<'_>> = splices
                .iter()
                .zip(&inserts)
                .map(|(splice, insert)| Replacement {
                    range: splice.start..splice.start + splice.old_len,
                    text: insert,
                })
                .collect();
            editor.apply_edits(id, &edits).unwrap();
        }
        let buffer = editor.buffer(id).unwrap();
        if buffer.revision() == revision {
            continue;
        }
        let after = buffer.text().clone();
        seen.server.change(Change {
            id: 9,
            before: sent,
            after: after.clone(),
            splices: buffer.edits_since(revision),
            revision: buffer.revision(),
        });
        revision = buffer.revision();
        last_hash = fnv(after.to_string().as_bytes());
        sent = after;
        version += 1;
    }
    let line = format!(" v{version} len=");
    seen.until(LIMIT, |seen| seen.logged(&line));
    let logged = seen.log.iter().find(|row| row.contains(&line)).unwrap();
    assert!(
        logged.ends_with(&format!("hash={last_hash:016x}")),
        "seed {seed}: {logged}"
    );
    assert!(!seen.logged("closed"), "no lone carriage return was made");
    seen.server.shut_down();
}

#[test]
fn a_followed_buffers_batches_keep_the_copy_in_utf_8() {
    a_followed_buffers_batches_keep_the_copy("utf-8", 3);
}

#[test]
fn a_followed_buffers_batches_keep_the_copy_in_utf_16() {
    a_followed_buffers_batches_keep_the_copy("utf-16", 4);
}

/// Documents past the writer queue's bound in all open one at a time, as room comes, and the
/// server is not stopped as one that did not read.
#[test]
fn many_large_documents_open_as_room_comes() {
    use oso_core::rope::Rope;
    use oso_lsp::server::Document;
    // It reads nothing for a while after the first open, so that what is sent waits in the
    // writer's queue.
    let mut script = syncing("utf-8");
    script["rules"]
        .as_array_mut()
        .unwrap()
        .push(json!({"method": "textDocument/didOpen", "nth": 1, "do": [{"sleep_ms": 1500}]}));
    let mut seen = started("large", &script, open());
    let text = Rope::from("x".repeat(7 << 20).as_str());
    for id in 0..12u64 {
        seen.server.open(Document {
            id,
            uri: format!("file:///w/p/large{id}.rs"),
            language_id: "rust".into(),
            text: text.clone(),
            revision: 0,
        });
    }
    seen.until(Duration::from_secs(60), |seen| {
        (0..12).all(|id| seen.logged(&format!("/large{id}.rs v1 ")))
    });
    assert!(!seen.logged("crashed: "), "{:?}", seen.log);
    seen.server.shut_down();
}

/// Random edits before a word, made by the editor, then a report for the version the document
/// opened as, which the server sends on the save after them: placed against that text on the
/// session thread, mapped forward through each batch, and taken by the buffer, it covers the
/// word where it is now (ADR 0138, step 6).
fn a_report_for_the_first_version_lands_on_the_word_after_random_batches(
    encoding: &str,
    seed: u64,
) {
    use oso_core::buffer::{Buffer, Replacement};
    use oso_core::editor::Editor;
    use oso_lsp::server::{Change, Diagnosed, Document};
    let first = "fn main() {\r\n    let é = \"日\u{1F600}\";\n}\n// marker\n";
    let name = format!("diagnosed-{encoding}-{seed}");
    // A file of the project `started` makes, as a report for one outside it is dropped.
    let uri = oso_lsp::uri::from_path(&scratch(&name).join("t.rs")).unwrap();
    let script = word_reported_on_save(encoding, &uri, first);
    let mut seen = started(&name, &script, open());
    let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", first));
    let id = editor.window().buffer();
    editor.buffer_mut(id).unwrap().set_followed(true);
    let mut revision = editor.buffer(id).unwrap().revision();
    let mut sent = editor.buffer(id).unwrap().text().clone();
    seen.server.open(Document {
        id: 9,
        uri,
        language_id: "rust".into(),
        text: sent.clone(),
        revision,
    });
    seen.until(LIMIT, |seen| seen.logged(" v1 "));
    let mut rng = Lcg(seed);
    // Fewer batches than the 16 texts kept, so that version 1 is still there.
    for _ in 0..12 {
        if rng.below(5) == 0 {
            let _ = editor.undo();
        } else {
            let edits = edits_before_the_word(&mut rng, &sent.to_string());
            editor.apply_edits(id, &edits).unwrap();
        }
        let buffer = editor.buffer(id).unwrap();
        if buffer.revision() == revision {
            continue;
        }
        let after = buffer.text().clone();
        seen.server.change(Change {
            id: 9,
            before: sent,
            after: after.clone(),
            splices: buffer.edits_since(revision),
            revision: buffer.revision(),
        });
        revision = buffer.revision();
        sent = after;
    }
    seen.server.save(9);
    seen.until(LIMIT, |seen| !seen.diagnosed.is_empty());
    let Some(Diagnosed::Open {
        id: document,
        revision: reported,
        diagnostics,
    }) = seen.diagnosed.pop()
    else {
        panic!("an open document's set");
    };
    assert_eq!(document, 9);
    // A batch more before the buffer takes it, which the buffer maps it through.
    editor
        .apply_edits(
            id,
            &[Replacement {
                range: 0..0,
                text: "// new\n",
            }],
        )
        .unwrap();
    let buffer = editor.buffer_mut(id).unwrap();
    assert!(
        buffer.take_diagnostics(reported, diagnostics),
        "seed {seed}"
    );
    let text = buffer.text().to_string();
    let set = buffer.diagnostics().unwrap();
    assert_eq!(set.len(), 1);
    let range = set.get(0).unwrap().range();
    assert_eq!(&text[range], "marker", "seed {seed}");
    assert_eq!(set.server(), "fake");
    seen.server.shut_down();
}

/// The fake server's script for the test above: on a save, a report for version 1 of `first`
/// over the word `marker`, in `encoding`.
fn word_reported_on_save(encoding: &str, uri: &str, first: &str) -> Value {
    use oso_lsp::position::{self, Encoding};
    let rope = oso_core::rope::Rope::from(first);
    let units = if encoding == "utf-8" {
        Encoding::Utf8
    } else {
        Encoding::Utf16
    };
    let at = first.find("marker").unwrap();
    let place = |offset| {
        let p = position::from_offset(&rope, offset, units);
        json!({"line": p.line, "character": p.character})
    };
    let mut script = syncing(encoding);
    script["rules"].as_array_mut().unwrap().push(json!({
        "method": "textDocument/didSave",
        "do": [{"notify": {"method": "textDocument/publishDiagnostics", "params": {
            "uri": uri,
            "version": 1,
            "diagnostics": [{
                "range": {"start": place(at), "end": place(at + "marker".len())},
                "severity": 1,
                "message": "a word",
            }],
        }}}],
    }));
    script
}

/// A random batch of edits of `text` before its `// marker` line.
fn edits_before_the_word(rng: &mut Lcg, text: &str) -> Vec<oso_core::buffer::Replacement<'static>> {
    let before_word = text.find("// marker").unwrap();
    let (splices, _) = random_batch(rng, &text[..before_word]);
    let inserts = ["x", "é", "\u{1F600}", "\n", "\r\n", ""];
    splices
        .iter()
        .filter(|splice| splice.start + splice.old_len < before_word)
        .map(|splice| oso_core::buffer::Replacement {
            range: splice.start..splice.start + splice.old_len,
            text: inserts[splice.new_len % 6],
        })
        .collect()
}

#[test]
fn a_report_for_the_first_version_lands_on_the_word_in_utf_8() {
    for seed in [5, 6, 7] {
        a_report_for_the_first_version_lands_on_the_word_after_random_batches("utf-8", seed);
    }
}

#[test]
fn a_report_for_the_first_version_lands_on_the_word_in_utf_16() {
    for seed in [8, 9, 10] {
        a_report_for_the_first_version_lands_on_the_word_after_random_batches("utf-16", seed);
    }
}

/// A report for a file outside the project, in a sibling whose name the root's is the start of,
/// through `..`, or whose name may hold secrets, is dropped and counted in `*LSP*`; one for a
/// file not open is kept, goes to the UI as a file's, as its set once it opens, when the UI
/// forgets the file's, and as a file's again once it closes, and the run's end forgets it.
#[test]
fn reports_outside_the_project_or_for_secrets_are_dropped_and_a_kept_one_follows_an_open() {
    use oso_lsp::server::{Diagnosed, Document};
    let report = |uri: &str| {
        json!({"notify": {"method": "textDocument/publishDiagnostics", "params": {
            "uri": uri,
            "diagnostics": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "message": "m",
            }],
        }}})
    };
    // The directory `started` makes for this test, so that the URIs can name files in it.
    let dir = scratch("diagnosed-dropped");
    let inside = |name: &str| oso_lsp::uri::from_path(&dir.join(name)).unwrap();
    let mut sibling = dir.clone().into_os_string();
    sibling.push("x");
    let sibling = oso_lsp::uri::from_path(&PathBuf::from(sibling).join("a.rs")).unwrap();
    let through = format!("{}/../a.rs", inside("src"));
    let mut script = syncing("utf-8");
    script["rules"].as_array_mut().unwrap().insert(
        0,
        json!({"method": "initialized", "do": [
            report("file:///elsewhere/a.rs"),
            report(&sibling),
            report(&through),
            report(&inside(".env")),
            report(&inside("later.rs")),
        ]}),
    );
    let mut seen = started("diagnosed-dropped", &script, open());
    seen.until(LIMIT, |seen| {
        seen.logged("dropped 4 diagnostics reports") && !seen.diagnosed.is_empty()
    });
    let later = oso_lsp::uri::to_path(&inside("later.rs")).unwrap();
    let file = |seen: &mut Seen| match seen.diagnosed.pop() {
        Some(Diagnosed::File { path, report, .. }) => (path, report.diagnostics.len()),
        other => panic!("a file's report, not {other:?}"),
    };
    assert_eq!(file(&mut seen), (later.clone(), 1));
    assert!(seen.diagnosed.is_empty(), "only the one kept");
    seen.server.open(Document {
        id: 4,
        uri: inside("later.rs"),
        language_id: "rust".into(),
        text: oso_core::rope::Rope::from("abc"),
        revision: 3,
    });
    // Its set, and the file's report forgotten: the document holds it now.
    seen.until(LIMIT, |seen| seen.diagnosed.len() >= 2);
    let opened = std::mem::take(&mut seen.diagnosed);
    assert!(
        opened.iter().any(|diagnosed| matches!(diagnosed,
            Diagnosed::Open { id: 4, revision: 3, diagnostics } if diagnostics.len() == 1)),
        "{opened:?}"
    );
    assert!(
        opened.iter().any(|diagnosed| matches!(diagnosed,
            Diagnosed::File { path, report, .. } if *path == later && report.diagnostics.is_empty())),
        "{opened:?}"
    );
    seen.server.close(4);
    seen.until(LIMIT, |seen| !seen.diagnosed.is_empty());
    assert_eq!(file(&mut seen), (later, 1), "a file's again once closed");
    assert!(
        !seen.log.iter().any(|line| line.contains(".env")),
        "never named"
    );
    seen.server.shut_down();
    seen.until(LIMIT, |seen| {
        matches!(seen.diagnosed.last(), Some(Diagnosed::Forgotten))
    });
}

/// A script that syncs in UTF-16, logs the positions asked about, and does `definition` for
/// `textDocument/definition`.
fn asking(definition: Value) -> Value {
    let mut script = syncing("utf-16");
    script["log_positions"] = json!(true);
    script["rules"]
        .as_array_mut()
        .unwrap()
        .extend(definition.as_array().cloned().unwrap_or_default());
    script
}

/// Opens the document `id` at `uri` holding `text`, of revision 7, and waits for it to go.
fn opened(seen: &mut Seen, id: u64, uri: &str, text: &str) {
    seen.server.open(oso_lsp::server::Document {
        id,
        uri: uri.into(),
        language_id: "rust".into(),
        text: oso_core::rope::Rope::from(text),
        revision: 7,
    });
    seen.until(LIMIT, |seen| seen.logged(" v1 "));
}

fn ask(seen: &Seen, kind: oso_lsp::server::AskKind, offset: usize) {
    seen.server.ask(oso_lsp::server::Ask {
        serial: offset as u64,
        document: 1,
        kind,
        revision: 7,
        offset,
        context: match kind {
            oso_lsp::server::AskKind::Completion => {
                oso_lsp::server::AskContext::Completion(oso_lsp::protocol::Invoked::Invoked)
            }
            _ => oso_lsp::server::AskContext::None,
        },
    });
}

#[test]
fn a_definition_is_asked_at_its_position_and_placed_in_the_open_text() {
    use oso_lsp::position::{Encoding, Position};
    use oso_lsp::server::{AskKind, Target};
    let dir = scratch("definition");
    let here = oso_lsp::uri::from_path(&dir.join("a.rs")).unwrap();
    let there = oso_lsp::uri::from_path(&dir.join("b.rs")).unwrap();
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    let script = asking(
        json!([{"method": "textDocument/definition", "do": [{"reply": [
            {"uri": here, "range": range(0, 3, 4)},
            {"targetUri": there, "targetRange": range(4, 0, 9), "targetSelectionRange": range(5, 2, 6)},
            {"uri": "https://example.com/x.rs", "range": range(0, 0, 1)},
        ]}]}]),
    );
    let mut seen = started("definition", &script, open());
    // `é` is one UTF-16 unit and two bytes: the position counts it once.
    opened(&mut seen, 1, &here, "fn a() {}\nlet é = a();\n");
    let offset = "fn a() {}\nlet é = ".len();
    ask(&seen, AskKind::Definition, offset);
    seen.until(LIMIT, |seen| {
        !seen.answers.is_empty() && seen.logged("stderr: asked textDocument/definition")
    });
    assert!(
        seen.logged("asked textDocument/definition 1:8"),
        "{:?}",
        seen.log
    );
    let answer = seen.answers.pop().unwrap();
    assert_eq!(
        (answer.document, answer.kind, answer.revision),
        (1, AskKind::Definition, 7)
    );
    let oso_lsp::server::Answered::Places(places) = answer.outcome.unwrap() else {
        panic!("places")
    };
    assert_eq!(places.len(), 2, "the URI that is not a file's is left out");
    // As a URI reads back: on Windows, with no `\\?\` prefix, which `canonicalize` gives.
    let read_back =
        |name: &str| oso_lsp::uri::to_path(&oso_lsp::uri::from_path(&dir.join(name)).unwrap());
    assert_eq!(Some(places[0].path.clone()), read_back("a.rs"));
    assert_eq!(
        places[0].target,
        Target::Open {
            document: 1,
            revision: 7,
            range: 3..4,
            start: Position::new(0, 3),
            encoding: Encoding::Utf16,
        }
    );
    assert_eq!(Some(places[1].path.clone()), read_back("b.rs"));
    assert_eq!(
        places[1].target,
        Target::Lines {
            start: Position::new(5, 2),
            end: Position::new(5, 6),
            encoding: Encoding::Utf16
        }
    );
    assert!(seen.logged("answered with 2 places"), "{:?}", seen.log);
    assert!(seen.logged("1 more left out"));
    assert!(
        !seen.log.iter().any(|line| line.contains("b.rs")),
        "no path is logged"
    );
    seen.server.shut_down();
}

#[test]
fn completions_are_asked_at_their_position_and_their_ranges_placed_in_the_open_text() {
    use oso_lsp::server::{Answered, AskKind};
    let dir = scratch("completion");
    let here = oso_lsp::uri::from_path(&dir.join("a.rs")).unwrap();
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    let script = asking(
        json!([{"method": "textDocument/completion", "do": [{"reply": {
            "isIncomplete": false,
            "items": [
                {"label": "émoji", "sortText": "2", "kind": 3, "textEdit": {"range": range(1, 4, 6), "newText": "émoji()"}},
                {"label": "élan", "sortText": "1", "insertText": "élan(${1:x})", "insertTextFormat": 2},
            ],
        }}]}]),
    );
    let mut seen = started("completion", &script, open());
    // `é` is one UTF-16 unit and two bytes.
    opened(&mut seen, 1, &here, "fn a() {}\nlet ém\n");
    let offset = "fn a() {}\nlet ém".len();
    ask(&seen, AskKind::Completion, offset);
    seen.until(LIMIT, |seen| {
        !seen.answers.is_empty() && seen.logged("stderr: asked textDocument/completion")
    });
    assert!(
        seen.logged("asked textDocument/completion 1:6"),
        "{:?}",
        seen.log
    );
    let answer = seen.answers.pop().unwrap();
    let Answered::Completion(completions) = answer.outcome.unwrap() else {
        panic!("completions")
    };
    assert_eq!(completions.revision, 7);
    assert!(!completions.incomplete);
    let first = &completions.items[0];
    assert_eq!(
        (first.label.as_str(), first.text.as_str()),
        ("élan", "élan(x)")
    );
    assert_eq!(first.range, None);
    let second = &completions.items[1];
    let start = "fn a() {}\nlet ".len();
    assert_eq!(
        second.range,
        Some(start..offset),
        "in bytes of the open text"
    );
    assert_eq!(second.kind, Some(3));
    assert!(seen.logged("answered with 2 completions"), "{:?}", seen.log);
    assert!(
        !seen.log.iter().any(|line| line.contains("émoji")),
        "no item is logged"
    );
    seen.server.shut_down();
}

#[test]
fn an_item_is_resolved_as_the_server_gave_it_and_its_edits_placed_in_the_open_text() {
    use oso_lsp::server::{Answered, AskContext, AskKind};
    let dir = scratch("resolve");
    let here = oso_lsp::uri::from_path(&dir.join("a.rs")).unwrap();
    let at = |line: u32, character: u32| json!({"line": line, "character": character});
    let script = json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                "completionProvider": {"triggerCharacters": ["."], "resolveProvider": true},
            }}}]},
            {"method": "textDocument/completion", "do": [{"reply": [
                {"label": "fmt", "data": {"id": 41}},
            ]}]},
            {"method": "completionItem/resolve", "do": [{"reply": {
                "label": "fmt",
                "additionalTextEdits": [{"range": {"start": at(0, 0), "end": at(0, 0)}, "newText": "use std::fmt;\n"}],
            }}]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "log_answers": false,
        "encoding": "utf-16",
    });
    let mut seen = started("resolve", &script, open());
    opened(&mut seen, 1, &here, "fn a() {}\nlet x = f\n");
    seen.until(LIMIT, |seen| seen.completing.is_some());
    let completing = seen.completing.clone().unwrap();
    assert_eq!(completing.triggers, ['.']);
    assert!(completing.resolves);
    let offset = "fn a() {}\nlet x = f".len();
    ask(&seen, AskKind::Completion, offset);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    let answer = seen.answers.pop().unwrap();
    assert!(matches!(answer.outcome, Ok(Answered::Completion(_))));
    // Its resolve names the answer by its ask's serial, and the item by its place.
    seen.server.ask(oso_lsp::server::Ask {
        serial: 99,
        document: 1,
        kind: AskKind::Resolve,
        revision: 7,
        offset,
        context: AskContext::Item {
            answer: answer.serial,
            index: 0,
        },
    });
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    let Ok(Answered::Resolved(resolved)) = seen.answers.pop().unwrap().outcome else {
        panic!("resolved")
    };
    assert_eq!(
        (resolved.answer, resolved.index, resolved.revision),
        (answer.serial, 0, 7)
    );
    assert_eq!(resolved.extra, [(0..0, "use std::fmt;\n".to_owned())]);
    // An item of an answer that is not the last is no one's to resolve.
    seen.server.ask(oso_lsp::server::Ask {
        serial: 100,
        document: 1,
        kind: AskKind::Resolve,
        revision: 7,
        offset,
        context: AskContext::Item {
            answer: answer.serial + 1,
            index: 0,
        },
    });
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert!(seen.answers.pop().unwrap().outcome.is_err());
    seen.server.shut_down();
}

#[test]
fn a_newer_ask_cancels_the_older_and_only_it_is_answered() {
    use oso_lsp::server::AskKind;
    let script = asking(json!([
        {"method": "textDocument/definition", "nth": 1, "do": [{"hang": true}]},
        {"method": "textDocument/definition", "nth": 2, "do": [{"reply": null}]},
    ]));
    let mut seen = started("cancel", &script, open());
    opened(&mut seen, 1, "file:///w/p/a.rs", "abc\n");
    ask(&seen, AskKind::Definition, 0);
    ask(&seen, AskKind::Definition, 1);
    seen.until(LIMIT, |seen| {
        !seen.answers.is_empty() && seen.logged("stderr: cancel")
    });
    std::thread::sleep(Duration::from_millis(200));
    if let Some(report) = seen.server.take_report() {
        seen.take(report);
    }
    assert_eq!(seen.answers.len(), 1, "{:?}", seen.answers);
    assert_eq!(
        seen.answers[0].outcome,
        Ok(oso_lsp::server::Answered::Places(Vec::new()))
    );
    assert_eq!(seen.answers[0].serial, 1, "the newer ask's");
    seen.until(LIMIT, |seen| {
        seen.logged("asked textDocument/definition 0:1")
    });
    seen.server.shut_down();
}

#[test]
fn an_ask_fails_on_an_old_text_an_error_a_crash_and_a_long_wait() {
    use oso_lsp::server::{AskFailure, AskKind};
    let script = asking(json!([
        {"method": "textDocument/references", "nth": 1, "do": [
            {"error": {"code": -32603, "message": "no index yet"}}
        ]},
        {"method": "textDocument/references", "nth": 2, "do": [{"hang": true}]},
        {"method": "textDocument/definition", "do": [{"exit": 3}]},
    ]));
    let mut seen = started("fails", &script, open());
    opened(&mut seen, 1, "file:///w/p/a.rs", "abc\n");
    seen.server.ask(oso_lsp::server::Ask {
        serial: 9,
        document: 1,
        kind: AskKind::Definition,
        revision: 6,
        offset: 0,
        context: oso_lsp::server::AskContext::None,
    });
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert_eq!(
        seen.answers.pop().unwrap().outcome,
        Err(AskFailure::Other(
            "the server does not have this text yet".into()
        ))
    );
    ask(&seen, AskKind::References, 0);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert_eq!(
        seen.answers.pop().unwrap().outcome,
        Err(AskFailure::Other("no index yet".into()))
    );
    assert!(seen.logged("failed with error -32603"));
    assert!(
        !seen.logged("no index yet"),
        "the server's text is not logged"
    );
    // The wait ends: the request is cancelled.
    let asked = Instant::now();
    ask(&seen, AskKind::References, 0);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert!(asked.elapsed() >= oso_lsp::limits::DEFINITION_WAIT);
    assert_eq!(
        seen.answers.pop().unwrap().outcome,
        Err(AskFailure::Overdue)
    );
    seen.until(LIMIT, |seen| seen.logged("stderr: cancel"));
    // A crash fails what waits.
    ask(&seen, AskKind::Definition, 0);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert_eq!(
        seen.answers.pop().unwrap().outcome,
        Err(AskFailure::Other("it stopped before it answered".into()))
    );
    seen.server.shut_down();
}

#[test]
fn an_ask_with_no_server_running_is_answered_at_once() {
    use oso_lsp::server::{AskFailure, AskKind};
    let mut seen = started(
        "no-run",
        &script(json!([]), json!([])),
        Box::new(|| Err("not installed".into())),
    );
    seen.until(LIMIT, |seen| seen.state() == Some(State::NotStarted));
    ask(&seen, AskKind::Definition, 0);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert_eq!(
        seen.answers[0].outcome,
        Err(AskFailure::Other("it does not run".into()))
    );
}

/// A description is asked at its position and comes back as the text it held, never logged;
/// one that waits past 3 s is cancelled and says so.
#[test]
fn a_description_is_asked_at_its_position_and_its_text_is_never_logged() {
    use oso_lsp::server::{Answered, AskFailure, AskKind};
    let script = asking(json!([
        {"method": "textDocument/hover", "nth": 1, "do": [{"reply": {"contents": {
            "kind": "markdown", "value": "```rust\nfn secret_name()\n```",
        }}}]},
        {"method": "textDocument/hover", "nth": 2, "do": [{"reply": null}]},
        {"method": "textDocument/hover", "nth": 3, "do": [{"hang": true}]},
    ]));
    let mut seen = started("hover", &script, open());
    opened(
        &mut seen,
        1,
        "file:///w/p/a.rs",
        "fn a() {}\nlet é = a();\n",
    );
    let offset = "fn a() {}\nlet é = ".len();
    ask(&seen, AskKind::Hover, offset);
    // The server's line of where it was asked may come after the answer.
    seen.until(LIMIT, |seen| {
        !seen.answers.is_empty() && seen.logged("stderr: asked textDocument/hover")
    });
    assert!(
        seen.logged("asked textDocument/hover 1:8"),
        "{:?}",
        seen.log
    );
    let answer = seen.answers.pop().unwrap();
    assert_eq!(answer.kind, AskKind::Hover);
    let Ok(Answered::Hover(Some(hover))) = answer.outcome else {
        panic!("a description");
    };
    assert_eq!(hover.text, "```rust\nfn secret_name()\n```");
    assert!(hover.markdown);
    assert!(seen.logged("bytes of description"));
    assert!(
        !seen.log.iter().any(|line| line.contains("secret_name")),
        "the text is not logged"
    );
    ask(&seen, AskKind::Hover, 0);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert_eq!(
        seen.answers.pop().unwrap().outcome,
        Ok(Answered::Hover(None))
    );
    let asked = Instant::now();
    ask(&seen, AskKind::Hover, 0);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert!(asked.elapsed() >= oso_lsp::limits::COMPLETION_WAIT);
    assert!(asked.elapsed() < oso_lsp::limits::DEFINITION_WAIT);
    assert_eq!(
        seen.answers.pop().unwrap().outcome,
        Err(AskFailure::Overdue)
    );
    seen.server.shut_down();
}

/// The fake server's script for code actions over the document at `here`: a report with a
/// diagnostic of a numeric code and `data`, an answer of each kind of action, and a resolve.
fn code_action_script(here: &str) -> Value {
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                "codeActionProvider": {"resolveProvider": true},
            }}}]},
            {"method": "textDocument/didOpen", "nth": 1, "do": [
                {"notify": {"method": "textDocument/publishDiagnostics", "params": {
                    "uri": here,
                    "version": 1,
                    "diagnostics": [
                        {"range": range(1, 4, 5), "severity": 1, "code": 2304, "message": "Cannot find", "data": {"fix": 7}},
                        {"range": range(0, 0, 2), "severity": 2, "message": "elsewhere"},
                    ],
                }}}
            ]},
            {"method": "textDocument/codeAction", "do": [{"reply": [
                {"title": "Import `x`", "kind": "quickfix", "isPreferred": true, "edit": {"documentChanges": [
                    {"textDocument": {"uri": here, "version": 1}, "edits": [{"range": range(0, 0, 0), "newText": "use x;\n"}]},
                ]}},
                {"title": "Rename to y", "kind": "refactor", "data": {"id": 1}},
                {"title": "Run it", "command": "fake.run"},
                {"title": "Elsewhere", "edit": {"changes": {"file:///w/other/b.rs": [{"range": range(0, 0, 0), "newText": "x"}]}}},
                {"title": "Old", "edit": {"documentChanges": [
                    {"textDocument": {"uri": here, "version": 0}, "edits": [{"range": range(0, 0, 0), "newText": "x"}]},
                ]}},
            ]}]},
            {"method": "codeAction/resolve", "do": [{"reply": {
                "title": "Rename to y",
                "edit": {"changes": {here: [{"range": range(1, 4, 5), "newText": "y"}]}},
            }}]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "log_positions": true,
        "encoding": "utf-16",
    })
}

#[test]
fn code_actions_are_asked_over_a_range_with_its_diagnostics_and_their_edits_placed() {
    use oso_lsp::server::{Answered, AskContext, AskKind};
    let dir = scratch("code-actions");
    let here = oso_lsp::uri::from_path(&dir.join("a.rs")).unwrap();
    let script = code_action_script(&here);
    let mut seen = started("code-actions", &script, open());
    opened(&mut seen, 1, &here, "fn a() {}\nlet x = 1;\n");
    seen.until(LIMIT, |seen| !seen.diagnosed.is_empty());
    let offset = "fn a() {}\nlet ".len();
    seen.server.ask(oso_lsp::server::Ask {
        serial: 5,
        document: 1,
        kind: AskKind::CodeAction,
        revision: 7,
        offset,
        context: AskContext::Range {
            start: offset,
            end: offset + 1,
        },
    });
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    // The range, and only the diagnostic there, as the server gave it.
    assert!(
        seen.logged(r#"asked textDocument/codeAction 1:4-1:5 1 diagnostics=[{"code":2304,"data":{"fix":7},"message":"Cannot find","range":{"end":{"character":5,"line":1},"start":{"character":4,"line":1}},"severity":1}]"#),
        "{}",
        seen.log.join("\n")
    );
    let Ok(Answered::CodeActions(actions)) = seen.answers.pop().unwrap().outcome else {
        panic!("code actions")
    };
    let [import, rename, run, elsewhere, old] = &actions.actions[..] else {
        panic!("{actions:?}")
    };
    assert!(import.preferred);
    let edit = import.edit.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(edit.open.len(), 1);
    assert_eq!((edit.open[0].document, edit.open[0].revision), (1, 7));
    assert_eq!(edit.open[0].edits, [(0..0, "use x;\n".to_owned())]);
    assert!(rename.edit.is_none() && rename.resolves);
    assert!(
        run.command.is_some() && !run.resolves,
        "a bare command is never resolved"
    );
    let elsewhere = elsewhere.edit.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(
        (elsewhere.open.len(), elsewhere.unsent.len()),
        (0, 1),
        "a file not open"
    );
    assert!(
        old.edit.as_ref().unwrap().is_err(),
        "made for an older text"
    );
    // The picked one resolves, by the answer's serial and its place.
    seen.server.ask(oso_lsp::server::Ask {
        serial: 6,
        document: 1,
        kind: AskKind::ResolveAction,
        revision: 7,
        offset,
        context: AskContext::Item {
            answer: 5,
            index: 1,
        },
    });
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    let Ok(Answered::ActionResolved(resolved)) = seen.answers.pop().unwrap().outcome else {
        panic!("resolved")
    };
    assert_eq!((resolved.answer, resolved.index), (5, 1));
    let edit = resolved.edit.unwrap().unwrap();
    assert_eq!(edit.open[0].edits, [(offset..offset + 1, "y".to_owned())]);
    seen.server.shut_down();
}

#[test]
fn an_edit_of_a_buffer_changed_while_the_server_answered_is_refused_and_64_diagnostics_go() {
    use oso_lsp::limits::ACTION_DIAGNOSTICS_MAX;
    use oso_lsp::server::{Answered, AskContext, AskKind, Change};
    let dir = scratch("code-actions-late");
    let here = oso_lsp::uri::from_path(&dir.join("a.rs")).unwrap();
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    let many: Vec<Value> = (0..ACTION_DIAGNOSTICS_MAX + 6)
        .map(|n| json!({"range": range(0, 0, 3), "message": format!("d{n}")}))
        .collect();
    let script = json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                "codeActionProvider": true,
            }}}]},
            {"method": "textDocument/didOpen", "nth": 1, "do": [
                {"notify": {"method": "textDocument/publishDiagnostics", "params": {
                    "uri": &here, "version": 1, "diagnostics": many,
                }}}
            ]},
            {"method": "textDocument/codeAction", "do": [
                {"sleep_ms": 600},
                {"reply": [{"title": "Late", "edit": {"changes": {here.clone(): [{"range": range(0, 0, 0), "newText": "x"}]}}}]},
            ]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "log_positions": true,
        "encoding": "utf-16",
    });
    let mut seen = started("code-actions-late", &script, open());
    opened(&mut seen, 1, &here, "let x = 1;\n");
    seen.until(LIMIT, |seen| !seen.diagnosed.is_empty());
    seen.server.ask(oso_lsp::server::Ask {
        serial: 5,
        document: 1,
        kind: AskKind::CodeAction,
        revision: 7,
        offset: 1,
        context: AskContext::Range { start: 1, end: 1 },
    });
    seen.until(LIMIT, |seen| seen.logged("asked textDocument/codeAction"));
    let at_most =
        format!("asked textDocument/codeAction 0:1-0:1 {ACTION_DIAGNOSTICS_MAX} diagnostics=");
    assert!(seen.logged(&at_most), "at most 64: {}", seen.log.join("\n"));
    // The buffer changes while the server answers.
    seen.server.change(Change {
        id: 1,
        before: oso_core::rope::Rope::from("let x = 1;\n"),
        after: oso_core::rope::Rope::from("let y = 1;\n"),
        splices: None,
        revision: 8,
    });
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    let Ok(Answered::CodeActions(actions)) = seen.answers.pop().unwrap().outcome else {
        panic!("code actions")
    };
    assert_eq!(
        actions.actions[0].edit,
        Some(Err(
            "a buffer it edits changed while the server answered".to_owned()
        ))
    );
    seen.server.shut_down();
}

#[test]
fn a_declared_command_runs_and_its_edit_waits_for_the_answer_an_undeclared_one_is_refused() {
    use oso_lsp::server::{Answered, AskContext, AskKind};
    let dir = scratch("code-action-command");
    let here = oso_lsp::uri::from_path(&dir.join("a.rs")).unwrap();
    let range = |line: u32, start: u32, end: u32| json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}});
    let script = json!({
        "rules": [
            {"method": "initialize", "do": [{"reply": {"capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {"openClose": true, "change": 2, "save": true},
                "codeActionProvider": true,
                "executeCommandProvider": {"commands": ["fake.fix"]},
            }}}]},
            {"method": "textDocument/codeAction", "do": [{"reply": [
                {"title": "Fix it", "command": {"title": "Fix it", "command": "fake.fix", "arguments": [3]}},
                {"title": "Run tests", "command": "fake.run"},
            ]}]},
            {"method": "workspace/executeCommand", "do": [
                {"request": {"method": "workspace/applyEdit", "params": {"label": "Fixed", "edit": {
                    "changes": {&here: [{"range": range(0, 4, 5), "newText": "y"}]},
                }}}},
                // A second while the first waits is refused.
                {"request": {"method": "workspace/applyEdit", "params": {"edit": {"changes": {}}}}},
                {"sleep_ms": 400},
                {"reply": null},
            ]},
            {"method": "shutdown", "do": [{"reply": null}]},
        ],
        "log_documents": true,
        "log_answers": true,
        "encoding": "utf-16",
    });
    let mut seen = started("code-action-command", &script, open());
    opened(&mut seen, 1, &here, "let x = 1;\n");
    seen.server.ask(oso_lsp::server::Ask {
        serial: 5,
        document: 1,
        kind: AskKind::CodeAction,
        revision: 7,
        offset: 4,
        context: AskContext::Range { start: 4, end: 4 },
    });
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    let Ok(Answered::CodeActions(actions)) = seen.answers.pop().unwrap().outcome else {
        panic!("code actions")
    };
    assert_eq!(actions.actions[0].command.as_deref(), Some("fake.fix"));
    let execute = |seen: &Seen, serial: u64, index: usize| {
        seen.server.ask(oso_lsp::server::Ask {
            serial,
            document: 1,
            kind: AskKind::ExecuteCommand,
            revision: 7,
            offset: 4,
            context: AskContext::Item { answer: 5, index },
        });
    };
    // One the server did not declare never reaches it.
    execute(&seen, 6, 1);
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    let refused = seen.answers.pop().unwrap().outcome;
    assert!(
        matches!(&refused, Err(oso_lsp::server::AskFailure::Other(why)) if why.contains("did not declare")),
        "{refused:?}"
    );
    // A declared one runs, with its arguments; its edit waits for the UI's answer.
    execute(&seen, 7, 0);
    seen.until(LIMIT, |seen| !seen.apply_edits.is_empty());
    let asked = seen.apply_edits.pop().unwrap();
    assert_eq!(asked.label.as_deref(), Some("Fixed"));
    let edit = asked.edit.unwrap();
    assert_eq!(edit.open[0].edits, [(4..5, "y".to_owned())]);
    assert!(seen.apply_edits.is_empty(), "only the first waits");
    seen.until(LIMIT, |seen| seen.logged(r#""applied":false"#));
    // An answer of another token is let go; the edit's own is answered.
    seen.server.answer_apply_edit(asked.token + 100, Ok(()));
    seen.server.answer_apply_edit(asked.token, Ok(()));
    seen.until(LIMIT, |seen| seen.logged(r#""applied":true"#));
    seen.until(LIMIT, |seen| !seen.answers.is_empty());
    assert!(matches!(
        seen.answers.pop().unwrap().outcome,
        Ok(Answered::Executed)
    ));
    seen.server.shut_down();
}
