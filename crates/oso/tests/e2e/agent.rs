//! The agent channel in the running app (ADR 0085): off until the person turns it on, a
//! connection let in only by the person's `y` after a quiet moment, commands refused by
//! their reach, nothing run while a buffer that may hold secrets is shown, kills kept from
//! the clipboard, a question the agent caused answered only by the person, a file it opened
//! still its own after a restart, and the channel closed by the person's command. The dev socket's keys stand in for the person's.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The channel's
//! limits and refusals have their own unit tests.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::control::{ControlDir, ControlStream, Socket};
use serde_json::{Value, json};

use super::handover::hand_over;
use super::harness::{App, FAKE_SECRET, Scratch};

/// How long the app takes no answer to the agent's question after a key, and a moment more.
const QUIET: Duration = Duration::from_millis(700);

/// How long the channel waits after the person's last key, and a moment more.
const AFTER_TYPING: Duration = Duration::from_millis(1200);

/// An agent on the channel's socket.
struct Agent {
    stream: ControlStream,
    reader: BufReader<ControlStream>,
    next_id: u64,
}

impl Agent {
    /// Connects once the channel listens.
    fn connect(app: &App) -> Self {
        let started = Instant::now();
        loop {
            let connected = ControlDir::open_existing()
                .and_then(|dir| dir.connect(Socket::Agent, Some(app.pid())));
            match connected {
                Ok((_, stream)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(30)))
                        .unwrap();
                    return Self {
                        reader: BufReader::new(stream.try_clone().unwrap()),
                        stream,
                        next_id: 1,
                    };
                }
                Err(e) => {
                    assert!(
                        started.elapsed().as_secs() < 10,
                        "the channel never listened: {e}"
                    );
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }

    fn send(&mut self, method: &str, params: Option<Value>) {
        let mut request = json!({ "id": self.next_id, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        self.next_id += 1;
        writeln!(self.stream, "{request}").unwrap();
    }

    /// The next response's result or error, or `None` once the channel closed.
    fn reply(&mut self) -> Option<Result<Value, String>> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => {
                let mut response: Value = serde_json::from_str(&line).unwrap();
                Some(match response["error"]["message"].as_str() {
                    Some(message) => Err(message.to_owned()),
                    None => Ok(response["result"].take()),
                })
            }
            Err(e) if e.kind() == ErrorKind::ConnectionReset => None,
            Err(e) => panic!("cannot read the channel: {e}"),
        }
    }

    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        self.send(method, params);
        self.reply().expect("the channel closed")
    }

    fn run(&mut self, name: &str, arg: Option<i64>, reach: &str) -> Result<Value, String> {
        self.call(
            "run",
            Some(json!({ "name": name, "arg": arg, "reach": reach })),
        )
    }
}

/// Turns the channel on as the person would, connects, and lets the agent in with `y` after a
/// quiet moment, checking on the way that a hasty `y` does not answer.
fn let_in(app: &mut App) -> Agent {
    app.run_command("agent-channel-start", None).unwrap();
    let state = app.wait_for("the channel", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.starts_with("Agent  "))
    });
    assert!(state["modeline"].is_string());
    let mut agent = Agent::connect(app);
    let version = thread::spawn(move || {
        let result = agent.call("version", None);
        (agent, result)
    });
    let state = app.wait_for("the question", |state| {
        state["prompts"].as_array().is_some_and(|p| !p.is_empty())
    });
    let label = state["prompts"][0]["prompt"].as_str().unwrap().to_owned();
    assert!(
        label.starts_with("Agent: let a process that reports itself as"),
        "{label}"
    );
    assert!(!label.contains("oso mcp"), "the test is not oso: {label}");
    let hasty = app.try_call("send-keys", Some(json!({ "keys": "y" })));
    assert!(hasty.unwrap_err().contains("takes no answer yet"));
    thread::sleep(QUIET);
    app.send_keys("y");
    let (agent, result) = version.join().unwrap();
    assert_eq!(result.unwrap()["protocol"], 1);
    thread::sleep(AFTER_TYPING);
    agent
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_agent_drives_only_once_the_person_lets_it_in_and_by_reach() {
    let scratch = Scratch::new("agent-reach");
    let file = scratch.write("notes.txt", b"one two\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let off =
        ControlDir::open_existing().and_then(|dir| dir.connect(Socket::Agent, Some(app.pid())));
    assert!(
        off.is_err(),
        "the channel is off until the person turns it on"
    );

    let mut agent = let_in(&mut app);
    assert_eq!(
        agent.run("forward-word", None, "look").unwrap()["command"],
        "forward-word"
    );
    assert_eq!(app.state()["cursor"]["column"], 3);
    assert!(
        agent
            .run("delete-char", None, "look")
            .unwrap_err()
            .contains("changes")
    );
    let exit = agent
        .run("save-buffers-kill-terminal", None, "change")
        .unwrap_err();
    assert!(exit.contains("needs a person"), "{exit}");
    assert!(
        agent
            .call("send-keys", Some(json!({ "keys": "C-x C-c" })))
            .unwrap_err()
            .starts_with("unknown method")
    );
    let state = agent.call("state", None).unwrap();
    assert_eq!(state["text"]["rows"][0], "one two");
    assert!(state.get("kill_ring").is_none());

    // The person types: the agent waits.
    app.send_keys("C-f");
    assert!(
        agent
            .run("forward-char", None, "look")
            .unwrap_err()
            .contains("typing")
    );
    thread::sleep(AFTER_TYPING);
    assert!(agent.run("forward-char", None, "look").is_ok());

    // It shows a buffer C-x b lists by its name, and makes none for a name that matches none.
    let switch = |agent: &mut Agent, name: &str| {
        let params = json!({ "name": "switch-to-buffer", "arg": name, "reach": "look" });
        agent.call("run", Some(params))
    };
    let switched = switch(&mut agent, "*Messages*");
    assert!(switched.is_ok(), "{switched:?}");
    assert_eq!(app.state()["buffer"]["name"], "*Messages*");
    let buffers = app.state()["buffers"].as_array().map(Vec::len);
    let missing = switch(&mut agent, "missing.md").unwrap_err();
    assert!(missing.contains("no buffer by that name"), "{missing}");
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "*Messages*");
    assert_eq!(
        state["buffers"].as_array().map(Vec::len),
        buffers,
        "no buffer made"
    );
    assert!(buffers.is_some());

    // The person turns it off: the agent's connection closes, and so does the socket.
    app.run_command("agent-channel-stop", None).unwrap();
    assert!(agent.reply().is_none(), "the connection closed");
    let state = app.wait_for("the channel off", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| !modeline.starts_with("Agent"))
    });
    assert!(state["modeline"].is_string());
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_agent_keeps_kills_from_the_clipboard_and_runs_nothing_beside_a_secret() {
    let scratch = Scratch::new("agent-secret");
    let file = scratch.write("notes.txt", b"one two\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let mut agent = let_in(&mut app);

    agent.run("mark-whole-buffer", None, "look").unwrap();
    agent.run("kill-ring-save", None, "look").unwrap();
    let state = app.state();
    assert_eq!(state["kill_ring"]["newest"]["pieces"], json!(["one two\n"]));
    thread::sleep(Duration::from_millis(300));
    let clipboard = app.call("clipboard", None);
    assert_ne!(
        clipboard["text"], "one two\n",
        "the agent's copy stays in oso"
    );

    app.open_file(&env);
    let state = app.wait_until_ready();
    assert_eq!(state["buffer"]["secret"], true);
    let refused = agent.run("forward-char", None, "look").unwrap_err();
    assert!(refused.contains("may hold secrets"), "{refused}");
    let state = agent.call("state", None).unwrap();
    assert_eq!(state["text"], Value::Null);
    assert!(!state.to_string().contains(FAKE_SECRET));

    // Once the person goes back, the agent cannot bring the secret buffer back by its name.
    app.send_keys("C-x b RET");
    app.wait_for("notes.txt", |state| state["buffer"]["name"] == "notes.txt");
    thread::sleep(AFTER_TYPING);
    let params = json!({ "name": "switch-to-buffer", "arg": ".env", "reach": "look" });
    let refused = agent.call("run", Some(params)).unwrap_err();
    assert!(refused.contains("may hold secrets"), "{refused}");
    assert_eq!(app.state()["buffer"]["name"], "notes.txt");
    let state = agent.call("state", None).unwrap();
    assert!(!state.to_string().contains(FAKE_SECRET));
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_question_the_agent_caused_waits_for_the_person() {
    let scratch = Scratch::new("agent-question");
    let file = scratch.write("notes.txt", b"one two\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    let mut agent = let_in(&mut app);

    agent.run("delete-char", None, "change").unwrap();
    std::fs::write(&file, b"changed on disk\n").unwrap();
    // Once the editor knows, a save asks. Waiting for the watcher keeps a save that would
    // find the change only as it writes out of the race.
    app.wait_for("the warning", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("changed on disk"))
    });
    let waiting = agent.run("save-buffer", None, "change").unwrap();
    let waiting = waiting["waiting"].as_str().unwrap();
    assert!(
        waiting.starts_with("waiting for the person: Agent: Save over notes.txt?"),
        "{waiting}"
    );
    let label = app.state()["prompts"][0]["prompt"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(label.starts_with("Agent: Save over"), "{label}");
    assert!(
        agent
            .run("forward-char", None, "look")
            .unwrap_err()
            .contains("waits for the person")
    );
    assert_eq!(agent.call("cancel", None).unwrap()["cancelled"], true);
    assert_eq!(app.state()["prompts"], json!([]));

    // A question the agent caused takes no answer once the channel stops: it closes.
    agent.run("save-buffer", None, "change").unwrap();
    assert!(
        app.state()["prompts"][0]["prompt"]
            .as_str()
            .unwrap()
            .starts_with("Agent: Save over")
    );
    app.run_command("agent-channel-stop", None).unwrap();
    assert_eq!(
        app.state()["prompts"],
        json!([]),
        "the agent's question closed with the channel"
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"changed on disk\n",
        "nothing saved over it"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn an_agent_opens_reads_and_edits_and_saves_its_own_files_only_with_a_yes() {
    let scratch = Scratch::new("agent-open");
    let notes = scratch.write("notes.txt", b"one\n");
    let other = scratch.write("other.txt", b"first\nsecond\nthird\n");
    let env = scratch.write(".env", format!("{FAKE_SECRET}\n").as_bytes());
    let link = scratch.symlink("plain.txt", &env);
    let alias = scratch.symlink("alias.txt", &other);
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    let mut agent = let_in(&mut app);
    let open = |agent: &mut Agent, path: &std::path::Path, line: Option<u64>| {
        agent.call("open", Some(json!({ "path": path, "line": line })))
    };

    let missing = scratch.path().join("missing.txt");
    assert!(
        open(&mut agent, &missing, None)
            .unwrap_err()
            .contains("no file")
    );
    assert!(
        open(&mut agent, &env, None)
            .unwrap_err()
            .contains("may hold secrets")
    );
    let linked = open(&mut agent, &link, None).unwrap_err();
    assert!(linked.contains("stays closed"), "{linked}");
    let aliased = open(&mut agent, &alias, None).unwrap_err();
    assert!(aliased.contains("another name"), "{aliased}");
    assert!(!missing.exists(), "the channel starts no new file");

    let opened = open(&mut agent, &other, Some(2)).unwrap();
    assert_eq!(opened["name"], "other.txt");
    let state = app.state();
    assert_eq!(state["buffer"]["name"], "other.txt");
    assert_eq!(state["cursor"]["row"], 1, "at its line");
    let names: Vec<_> = state["buffers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].clone())
        .collect();
    assert!(
        !names.contains(&json!("plain.txt")),
        "the linked secret was dropped: {names:?}"
    );
    let text = agent
        .call("text", Some(json!({ "buffer": opened["buffer"] })))
        .unwrap();
    assert_eq!(text["rows"], json!(["first", "second", "third", ""]));

    agent.run("insert", None, "change").unwrap_err();
    let inserted = agent.call(
        "run",
        Some(json!({ "name": "insert", "arg": "new ", "reach": "change" })),
    );
    assert!(inserted.is_ok(), "{inserted:?}");
    let waiting = agent.run("save-buffer", None, "change").unwrap();
    let waiting = waiting["waiting"].as_str().unwrap();
    assert!(
        waiting.contains("Agent: Save ") && waiting.contains("other.txt, which the agent opened?"),
        "{waiting}"
    );
    assert_eq!(
        std::fs::read(&other).unwrap(),
        b"first\nsecond\nthird\n",
        "not yet"
    );
    thread::sleep(QUIET);
    app.send_keys("y");
    app.wait_for("the save", |state| state["buffer"]["modified"] == false);
    assert_eq!(
        std::fs::read(&other).unwrap(),
        b"first\nnew second\nthird\n"
    );
}

/// What the flow's agent sends `oso mcp`, a line each: it starts, lists the tools, looks,
/// tries what needs a person, and reads the state.
fn mcp_requests() -> String {
    let request = |id: u64, method: &str, params: Value| {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
    };
    let tool = |id: u64, name: &str, arguments: Value| {
        request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    };
    let lines = [
        request(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        request(2, "tools/list", json!({})),
        tool(3, "look", json!({ "name": "forward-word" })),
        tool(4, "change", json!({ "name": "save-buffers-kill-terminal" })),
        tool(5, "state", json!({})),
    ];
    format!("{}\n", lines.join("\n"))
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_file_the_person_visited_that_reloads_from_elsewhere_saves_for_the_agent_only_with_a_yes() {
    let scratch = Scratch::new("agent-relinked");
    let first = scratch.write("first.txt", b"first\n");
    let elsewhere = scratch.write("elsewhere.txt", b"elsewhere\n");
    let link = scratch.symlink("notes.txt", &first);
    let mut app = App::start(Some(&link), scratch.path());
    app.wait_until_ready();
    let mut agent = let_in(&mut app);

    // The link now leads elsewhere, and the watcher reads the file again.
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    app.wait_for("the reload", |state| {
        state["text"]["rows"][0] == "elsewhere"
    });

    let inserted = agent.call(
        "run",
        Some(json!({ "name": "insert", "arg": "new ", "reach": "change" })),
    );
    assert!(inserted.is_ok(), "{inserted:?}");
    let waiting = agent.run("save-buffer", None, "change").unwrap();
    let waiting = waiting["waiting"].as_str().unwrap().to_owned();
    assert!(
        waiting.contains("elsewhere.txt, which the agent opened?"),
        "{waiting}"
    );
    assert_eq!(
        std::fs::read(&elsewhere).unwrap(),
        b"elsewhere\n",
        "nothing written yet"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn oso_mcp_drives_the_editor_once_the_person_lets_it_in() {
    use std::io::Read;
    let scratch = Scratch::new("agent-mcp");
    let file = scratch.write("notes.txt", b"one two\n");
    let mut app = App::start(Some(&file), scratch.path());
    app.wait_until_ready();
    app.run_command("agent-channel-start", None).unwrap();
    app.wait_for("the channel", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.starts_with("Agent  "))
    });
    let requests = scratch.write("requests.jsonl", mcp_requests().as_bytes());
    let mut bridge = app.mcp(&requests);

    // The first tool call asks the person, who lets it in after a quiet moment.
    let state = app.wait_for("the question", |state| {
        state["prompts"]
            .as_array()
            .is_some_and(|prompts| !prompts.is_empty())
    });
    let label = state["prompts"][0]["prompt"].as_str().unwrap().to_owned();
    assert!(
        label.contains("through oso mcp"),
        "the bridge runs this program: {label}"
    );
    thread::sleep(QUIET);
    app.send_keys("y");

    let mut output = String::new();
    bridge
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    assert!(bridge.wait().unwrap().success());
    let responses: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        responses.len(),
        5,
        "no response to the notification: {output}"
    );
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "oso");
    assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 7);
    let text = |response: &Value| {
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    // The person typed `y` just now, so the agent waits; either way nothing ran unasked.
    let looked = &responses[2];
    assert!(
        looked["result"]["isError"] == false || text(looked).contains("typing"),
        "{looked}"
    );
    assert_eq!(responses[3]["result"]["isError"], true);
    assert!(text(&responses[3]).contains("needs a person"));
    let state: Value = serde_json::from_str(&text(&responses[4])).unwrap();
    let buffers = state["buffers"].as_array().unwrap();
    assert!(
        buffers.iter().any(|buffer| buffer["name"] == "notes.txt"),
        "{buffers:?}"
    );
    assert_eq!(app.state()["prompts"], json!([]), "the editor runs on");
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_question_that_follows_the_persons_yes_closes_when_the_agent_goes() {
    let scratch = Scratch::new("agent-follow-up");
    let notes = scratch.write("notes.txt", b"one\n");
    let other = scratch.write("other.txt", b"first\n");
    let mut app = App::start(Some(&notes), scratch.path());
    app.wait_until_ready();
    let mut agent = let_in(&mut app);
    agent.call("open", Some(json!({ "path": other }))).unwrap();
    agent
        .call(
            "run",
            Some(json!({ "name": "insert", "arg": "x", "reach": "change" })),
        )
        .unwrap();
    // The file changes under the agent's edit.
    let new = other.with_extension("new");
    std::fs::write(&new, b"changed on disk\n").unwrap();
    std::fs::rename(&new, &other).unwrap();
    app.wait_for("the warning", |state| {
        state["modeline"]
            .as_str()
            .is_some_and(|modeline| modeline.contains("changed on disk"))
    });

    // Each save asks the person first, as the agent opened the file; the person's yes then
    // finds the file changed, and asks again, for the agent.
    let mut follow_up = None;
    for _ in 0..3 {
        thread::sleep(AFTER_TYPING);
        let asked = agent.run("save-buffer", None, "change").unwrap();
        assert!(
            asked["waiting"]
                .as_str()
                .is_some_and(|w| w.contains("which the agent opened?")),
            "{asked}"
        );
        thread::sleep(QUIET);
        app.send_keys("y");
        let state = app.state();
        if let Some(label) = state["prompts"][0]["prompt"].as_str() {
            follow_up = Some(label.to_owned());
            break;
        }
    }
    let follow_up = follow_up.expect("the person's yes asked again");
    assert!(
        follow_up.starts_with("Agent: "),
        "the follow-up is the agent's: {follow_up}"
    );

    drop(agent);
    let state = app.wait_for("the question closed", |state| state["prompts"] == json!([]));
    assert_eq!(state["prompts"], json!([]));
    assert_eq!(
        std::fs::read(&other).unwrap(),
        b"changed on disk\n",
        "nothing saved over it"
    );
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn the_files_an_agent_opened_still_ask_before_its_saves_after_a_restart() {
    let scratch = Scratch::new("agent-restart");
    let notes = scratch.write("notes.txt", b"mine\n");
    let read_again = scratch.write("read-again.txt", b"as on disk\n");
    let carried = scratch.write("carried.txt", b"one\n");
    let mut old = App::start(Some(&notes), scratch.path());
    old.wait_until_ready();
    let mut agent = let_in(&mut old);
    for path in [&read_again, &carried] {
        let opened = agent.call("open", Some(json!({ "path": path })));
        assert!(opened.is_ok(), "{opened:?}");
    }
    let inserted = agent.call(
        "run",
        Some(json!({ "name": "insert", "arg": "new ", "reach": "change" })),
    );
    assert!(inserted.is_ok(), "{inserted:?}");
    drop(agent);

    let mut new = App::start(None, scratch.path());
    new.wait_until_ready();
    hand_over(&mut old, &mut new);
    // Read again from its file, or carried unsaved: both are still the agent's.
    let state = new.call("export-state", None);
    let by_path = |path: &std::path::Path| {
        state["buffers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|buffer| buffer["path"].as_str() == path.to_str())
            .cloned()
            .unwrap_or_else(|| panic!("no {}: {state}", path.display()))
    };
    assert_eq!(by_path(&read_again)["opened_by_agent"], true);
    assert!(by_path(&read_again)["text"].is_null(), "read again");
    assert_eq!(by_path(&carried)["opened_by_agent"], true);
    assert!(by_path(&carried)["text"].is_object(), "carried");
    assert_eq!(by_path(&notes).get("opened_by_agent"), None);

    let mut agent = let_in(&mut new);
    let waiting = agent.run("save-buffer", None, "change").unwrap();
    let waiting = waiting["waiting"].as_str().unwrap();
    assert!(
        waiting.contains("carried.txt, which the agent opened?"),
        "{waiting}"
    );
    assert_eq!(std::fs::read(&carried).unwrap(), b"one\n", "not yet");
    thread::sleep(QUIET);
    new.send_keys("y");
    new.wait_for("the save", |state| state["buffer"]["modified"] == false);
    assert_eq!(std::fs::read(&carried).unwrap(), b"new one\n");
}
