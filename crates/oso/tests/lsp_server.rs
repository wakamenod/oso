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
    });
    seen.server.change(Change {
        id: 1,
        before: Rope::from("a"),
        after: Rope::from("ab"),
        splices: None,
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
        });
    }
    seen.until(Duration::from_secs(60), |seen| {
        (0..12).all(|id| seen.logged(&format!("/large{id}.rs v1 ")))
    });
    assert!(!seen.logged("crashed: "), "{:?}", seen.log);
    seen.server.shut_down();
}
