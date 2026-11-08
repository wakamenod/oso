//! `oso mcp`'s JSON-RPC against a stand-in channel.
//!
//! Must not depend on: a socket or a running oso; the stand-in answers in its place.

use std::cell::RefCell;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Bridge, Connect, Failed, Link, MAX_LINE, serve};
use crate::channel::protocol::PROTOCOL_VERSION;

/// What the stand-in channel was asked, and how it answers.
struct Asked {
    calls: Vec<(String, Value)>,
    connects: usize,
    /// The next call fails as a broken connection.
    breaks: bool,
    /// The protocol its `version` says.
    protocol: u32,
}

impl Default for Asked {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            connects: 0,
            breaks: false,
            protocol: PROTOCOL_VERSION,
        }
    }
}

struct StandIn(Rc<RefCell<Asked>>);

impl Link for StandIn {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        let mut asked = self.0.borrow_mut();
        if method == "version" {
            return Ok(json!({ "protocol": asked.protocol }));
        }
        asked.calls.push((method.to_owned(), params.clone()));
        if std::mem::take(&mut asked.breaks) {
            return Err(Failed::Broken("oso closed the connection".into()));
        }
        if method == "run" && params["name"] == "save-buffers-kill-terminal" {
            return Err(Failed::Refused(
                "save-buffers-kill-terminal needs a person at the keyboard".into(),
            ));
        }
        Ok(json!({ "method": method, "text": "a\u{202e}b" }))
    }
}

fn bridge(asked: &Rc<RefCell<Asked>>) -> Bridge {
    let asked = Rc::clone(asked);
    let connect: Connect = Box::new(move || {
        asked.borrow_mut().connects += 1;
        Ok(Box::new(StandIn(Rc::clone(&asked))) as Box<dyn Link>)
    });
    Bridge::new(connect)
}

fn response(bridge: &mut Bridge, request: &Value) -> Value {
    let line = bridge.respond(&request.to_string()).expect("a response");
    serde_json::from_str(&line).unwrap()
}

fn call(tool: &str, arguments: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
}

fn text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn initializes_without_connecting_and_lists_its_tools() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let init = response(
        &mut bridge,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "oso");
    let newest = response(
        &mut bridge,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }),
    );
    assert_eq!(newest["result"]["protocolVersion"], "2025-06-18");
    let list = response(
        &mut bridge,
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    );
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["state", "text", "commands", "look", "change", "open"]
    );
    let hint = |name: &str, hint: &str| {
        tools.iter().find(|tool| tool["name"] == name).unwrap()["annotations"][hint].clone()
    };
    for read in ["state", "text", "commands"] {
        assert_eq!(hint(read, "readOnlyHint"), true, "{read}");
    }
    assert_eq!(hint("look", "readOnlyHint"), false);
    assert_eq!(hint("open", "readOnlyHint"), false);
    assert_eq!(hint("change", "destructiveHint"), true);
    assert_eq!(
        asked.borrow().connects,
        0,
        "nothing asks the person until a tool runs"
    );
}

#[test]
fn a_notification_runs_nothing_and_gets_no_response() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert!(bridge.respond(&note.to_string()).is_none());
    let mut call = call("change", &json!({ "name": "insert", "arg": "x" }));
    call.as_object_mut().unwrap().remove("id");
    assert!(bridge.respond(&call.to_string()).is_none());
    assert_eq!(
        asked.borrow().connects,
        0,
        "a tools/call with no id runs nothing"
    );
}

#[test]
fn tools_become_the_channels_methods_with_their_reach() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    response(
        &mut bridge,
        &call("look", &json!({ "name": "forward-word", "arg": 2 })),
    );
    response(
        &mut bridge,
        &call("change", &json!({ "name": "insert", "arg": "hi" })),
    );
    response(
        &mut bridge,
        &call("open", &json!({ "path": "/tmp/a.md", "line": 3 })),
    );
    response(&mut bridge, &call("state", &json!({})));
    let calls = asked.borrow().calls.clone();
    let run = |name: &str, arg: Value, reach: &str| {
        (
            "run".to_owned(),
            json!({ "name": name, "arg": arg, "reach": reach }),
        )
    };
    assert_eq!(calls[0], run("forward-word", json!(2), "look"));
    assert_eq!(calls[1], run("insert", json!("hi"), "change"));
    assert_eq!(
        calls[2],
        ("open".to_owned(), json!({ "path": "/tmp/a.md", "line": 3 }))
    );
    assert_eq!(calls[3].0, "state");
    assert_eq!(asked.borrow().connects, 1, "one connection for all");
}

#[test]
fn a_reach_cannot_be_given_by_the_agent() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let sneaky = response(
        &mut bridge,
        &call("look", &json!({ "name": "delete-char", "reach": "change" })),
    );
    assert_eq!(sneaky["error"]["code"], -32602);
    assert!(asked.borrow().calls.is_empty());
}

#[test]
fn refusals_are_tool_errors_keep_the_connection_and_results_hide_nothing() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let refused = response(
        &mut bridge,
        &call("change", &json!({ "name": "save-buffers-kill-terminal" })),
    );
    assert_eq!(refused["result"]["isError"], true);
    assert!(text(&refused).contains("needs a person"));
    let line = bridge
        .respond(&call("state", &json!({})).to_string())
        .unwrap();
    assert!(
        line.contains("\\u202e"),
        "bidi controls go out escaped: {line}"
    );
    assert_eq!(asked.borrow().connects, 1, "a refusal keeps the connection");
}

#[test]
fn a_name_echoed_in_an_error_goes_out_escaped() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let line = bridge
        .respond(&call("no\u{202e}such\u{85}", &json!({})).to_string())
        .unwrap();
    assert!(
        !line.contains('\u{202e}') && !line.contains('\u{85}'),
        "{line}"
    );
    let long_id = json!({ "jsonrpc": "2.0", "id": "i".repeat(300), "method": "ping" });
    let refused: Value =
        serde_json::from_str(&bridge.respond(&long_id.to_string()).unwrap()).unwrap();
    assert_eq!(refused["error"]["code"], -32600);
}

#[test]
fn a_channel_of_another_protocol_is_refused_and_asked_again() {
    let asked: Rc<RefCell<Asked>> = Rc::default();
    asked.borrow_mut().protocol = PROTOCOL_VERSION + 1;
    let mut bridge = bridge(&asked);
    let refused = response(&mut bridge, &call("state", &json!({})));
    assert_eq!(refused["result"]["isError"], true);
    assert!(text(&refused).contains("protocol"), "{}", text(&refused));
    assert!(asked.borrow().calls.is_empty(), "nothing ran on it");
    asked.borrow_mut().protocol = PROTOCOL_VERSION;
    response(&mut bridge, &call("state", &json!({})));
    assert_eq!(asked.borrow().connects, 2);
}

#[test]
fn a_broken_connection_is_made_again_on_the_next_call() {
    let asked: Rc<RefCell<Asked>> = Rc::default();
    let mut bridge = bridge(&asked);
    response(&mut bridge, &call("state", &json!({})));
    asked.borrow_mut().breaks = true;
    let broken = response(&mut bridge, &call("state", &json!({})));
    assert_eq!(broken["result"]["isError"], true);
    response(&mut bridge, &call("state", &json!({})));
    assert_eq!(asked.borrow().connects, 2);
}

#[test]
fn unknown_methods_tools_and_bad_lines_are_errors() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let unknown = response(&mut bridge, &json!({ "id": 1, "method": "resources/list" }));
    assert_eq!(unknown["error"]["code"], -32601);
    let tool = response(&mut bridge, &call("send-keys", &json!({ "keys": "y" })));
    assert_eq!(tool["error"]["code"], -32602);
    let parse: Value = serde_json::from_str(&bridge.respond("{nope").unwrap()).unwrap();
    assert_eq!(parse["error"]["code"], -32700);
}

#[test]
fn serves_each_line_until_input_ends_and_refuses_an_overlong_one() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let input = format!(
        "{}\n\n{}\n",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    );
    let mut output = Vec::new();
    assert_eq!(
        serve(&mut bridge, Box::new(Cursor::new(input)), true, &mut output),
        0
    );
    let output = String::from_utf8(output).unwrap();
    assert_eq!(output.lines().count(), 1, "{output}");
    let long = format!("{}\n", "x".repeat(MAX_LINE + 10));
    let mut output = Vec::new();
    assert_eq!(
        serve(&mut bridge, Box::new(Cursor::new(long)), true, &mut output),
        1
    );
    let refused: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(refused["error"]["code"], -32700);
}

/// Whether the call has started waiting, and whether its connection was closed.
type Waited = Arc<(Mutex<(bool, bool)>, Condvar)>;

/// A channel whose calls wait until it is closed, as one waits for the person's yes.
struct Waiting(Waited);

impl Link for Waiting {
    fn call(&mut self, method: &str, _: Value) -> Result<Value, Failed> {
        assert_eq!(method, "version", "{method} ran after the agent went");
        let (state, woken) = &*self.0;
        let mut guard = state.lock().unwrap();
        guard.0 = true;
        woken.notify_all();
        let (guard, timeout) = woken
            .wait_timeout_while(guard, Duration::from_secs(10), |(_, closed)| !*closed)
            .unwrap();
        assert!(!timeout.timed_out(), "nothing closed the waiting call");
        assert!(guard.1);
        Err(Failed::Broken("oso closed the connection".into()))
    }

    fn closer(&self) -> Option<Box<dyn Fn() + Send>> {
        let shared = Arc::clone(&self.0);
        Some(Box::new(move || {
            shared.0.lock().unwrap().1 = true;
            shared.1.notify_all();
        }))
    }
}

/// stdin that gives one line, and ends once the call it asks for waits.
struct EndsWhileItWaits {
    line: Cursor<Vec<u8>>,
    waited: Waited,
}

impl std::io::Read for EndsWhileItWaits {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.line.read(buf)?;
        if read > 0 {
            return Ok(read);
        }
        let (state, woken) = &*self.waited;
        let guard = state.lock().unwrap();
        drop(woken.wait_while(guard, |(started, _)| !*started).unwrap());
        Ok(0)
    }
}

#[test]
fn a_call_that_waits_ends_when_the_agents_stdin_does() {
    let waited: Waited = Arc::default();
    let link = Arc::clone(&waited);
    let connect: Connect =
        Box::new(move || Ok(Box::new(Waiting(Arc::clone(&link))) as Box<dyn Link>));
    let mut bridge = Bridge::new(connect);
    let line = format!(
        "{}\n",
        call("change", &json!({ "name": "insert", "arg": "x" }))
    );
    let input = EndsWhileItWaits {
        line: Cursor::new(line.into_bytes()),
        waited: Arc::clone(&waited),
    };
    let mut output = Vec::new();
    let input = Box::new(std::io::BufReader::new(input));
    assert_eq!(serve(&mut bridge, input, true, &mut output), 0);
    let answered: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        answered["result"]["isError"], true,
        "the call that waited ended"
    );
    assert!(waited.0.lock().unwrap().1, "the connection was closed");
}
