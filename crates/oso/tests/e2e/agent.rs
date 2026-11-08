//! The agent channel in the running app (ADR 0085): off until the person turns it on, a
//! connection let in only by the person's `y` after a quiet moment, commands refused by
//! their reach, nothing run while a buffer that may hold secrets is shown, kills kept from
//! the clipboard, a question the agent caused answered only by the person, and the channel
//! closed by the person's command. The dev socket's keys stand in for the person's.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The channel's
//! limits and refusals have their own unit tests.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::control::{ControlDir, ControlStream, Socket};
use serde_json::{Value, json};

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
    // The first save finds the file changed and keeps it; the next asks.
    agent.run("save-buffer", None, "change").unwrap();
    app.wait_for("the save", |state| state["buffer"]["saving"] == false);
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
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"changed on disk\n",
        "nothing saved over it"
    );
}
