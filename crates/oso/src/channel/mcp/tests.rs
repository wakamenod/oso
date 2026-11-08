//! `oso mcp`'s JSON-RPC against a stand-in channel.
//!
//! Must not depend on: a socket or a running oso; the stand-in answers in its place.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};

use super::{Bridge, Failed, Link, serve};

/// What the stand-in channel was asked, and how it answers.
#[derive(Default)]
struct Asked {
    calls: Vec<(String, Value)>,
    connects: usize,
    /// The next call fails as a broken connection.
    breaks: bool,
}

struct StandIn(Rc<RefCell<Asked>>);

impl Link for StandIn {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        let mut asked = self.0.borrow_mut();
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

type Connect = Box<dyn FnMut() -> Result<Box<dyn Link>, String>>;

fn bridge(asked: &Rc<RefCell<Asked>>) -> Bridge<Connect> {
    let asked = Rc::clone(asked);
    Bridge::new(Box::new(move || {
        asked.borrow_mut().connects += 1;
        Ok(Box::new(StandIn(Rc::clone(&asked))) as Box<dyn Link>)
    }))
}

fn response(bridge: &mut Bridge<Connect>, request: &Value) -> Value {
    let line = bridge.handle(&request.to_string()).expect("a response");
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
    let note = r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#;
    assert!(
        bridge.handle(note).is_none(),
        "a notification gets no response"
    );
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
fn refusals_are_tool_errors_and_results_hide_nothing() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let refused = response(
        &mut bridge,
        &call("change", &json!({ "name": "save-buffers-kill-terminal" })),
    );
    assert_eq!(refused["result"]["isError"], true);
    let text = refused["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("needs a person"), "{text}");
    let line = bridge
        .handle(&call("state", &json!({})).to_string())
        .unwrap();
    assert!(
        line.contains("\\u202e"),
        "bidi controls go out escaped: {line}"
    );
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
    let parse: Value = serde_json::from_str(&bridge.handle("{nope").unwrap()).unwrap();
    assert_eq!(parse["error"]["code"], -32700);
}

#[test]
fn serves_each_line_until_input_ends() {
    let asked = Rc::default();
    let mut bridge = bridge(&asked);
    let input = format!(
        "{}\n\n{}\n",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    );
    let mut output = Vec::new();
    assert_eq!(serve(&mut bridge, input.as_bytes(), &mut output), 0);
    let output = String::from_utf8(output).unwrap();
    assert_eq!(output.lines().count(), 1, "{output}");
    let long = format!("{}\n", "x".repeat(super::MAX_LINE + 10));
    let mut output = Vec::new();
    assert_eq!(serve(&mut bridge, long.as_bytes(), &mut output), 1);
}
