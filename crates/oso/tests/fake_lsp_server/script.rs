//! The fake server's script: what it does at its start and for each message it reads, as
//! plain values the I/O loop carries out.
//!
//! Must not depend on: the process's streams; `main.rs` reads and writes them.
//!
//! A script is a JSON object:
//!
//! ```json
//! {
//!   "start": [{"notify": {"method": "window/logMessage", "params": {"type": 3, "message": "hi"}}}],
//!   "rules": [
//!     {"method": "initialize", "do": [{"reply": {"capabilities": {"positionEncoding": "utf-8"}}}]},
//!     {"method": "textDocument/hover", "nth": 2, "do": [{"hang": true}]},
//!     {"method": "shutdown", "do": [{"reply": null}]}
//!   ]
//! }
//! ```
//!
//! A rule matches a method, and with `nth` only its nth message, from 1; the first rule that
//! matches is used. With `"log_answers": true`, each answer the client gives the server's
//! requests is written to the standard error as a line, `answer: <JSON>`, for a test to read.
//! With `"log_documents": true`, it keeps its own copy of each document the client opens,
//! applying each change with positions in `"encoding"` (`"utf-8"`, or `"utf-16"` unless it says),
//! and writes `doc <uri> v<version> len=<bytes> hash=<FNV-1a 64, hex>` after each open and
//! change, and `closed <uri>` after a close. With `"log_positions": true`, a request with a
//! `position` writes `asked <method> <line>:<character>`, and `$/cancelRequest` writes
//! `cancel <id>`. A request no rule matches gets `MethodNotFound`, a notification nothing,
//! and `exit` ends the server with 0. The actions:
//!
//! - `reply`: the request's result; `error`: its error, `{"code", "message"}`;
//! - `notify` and `request`: a message to the client, `{"method", "params"}`;
//! - `raw`: these characters as they are, such as a broken header;
//! - `oversized`: a well-formed message of this many bytes, spaces then `null`;
//! - `flood`: `{"count", "method"}`, that many notifications at once;
//! - `hang`: no answer, ever; `sleep_ms`: a pause before the next action;
//! - `stderr`: `{"text", "repeat"}` on the standard error;
//! - `exit`: the server ends with this code.

use serde_json::{Map, Value, json};

/// One thing the server does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Writes these bytes to the standard output.
    Out(Vec<u8>),
    /// Writes these bytes to the standard error.
    Err(Vec<u8>),
    /// Waits this many milliseconds.
    Sleep(u64),
    /// Ends the server with this code.
    Exit(i32),
}

/// An action as the script names it, before a message gives it an id.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Reply(Value),
    Error(Value),
    Notify(Value),
    Request(Value),
    Raw(String),
    Oversized(usize),
    Flood { count: usize, method: String },
    Hang,
    Sleep(u64),
    Stderr { text: String, repeat: usize },
    Exit(i32),
}

#[derive(Debug)]
struct Rule {
    method: String,
    nth: Option<usize>,
    steps: Vec<Step>,
}

/// A parsed script and the messages it has seen.
#[derive(Debug)]
pub struct Script {
    start: Vec<Step>,
    rules: Vec<Rule>,
    /// How many messages of each method came, in order of first arrival.
    seen: Vec<(String, usize)>,
    /// The id of the next request the server sends.
    next_id: u64,
    /// Answers to the server's requests go to the standard error.
    log_answers: bool,
    /// Each document's text, kept and logged.
    log_documents: bool,
    /// The positions asked about, and cancels, go to the standard error.
    log_positions: bool,
    utf8: bool,
    documents: std::collections::HashMap<String, String>,
}

impl Script {
    /// Reads a script.
    ///
    /// # Errors
    ///
    /// What is wrong with it, for the test that wrote it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let start = match value.get("start") {
            Some(steps) => parse_steps(steps)?,
            None => Vec::new(),
        };
        let rules = match value.get("rules") {
            Some(Value::Array(rules)) => rules.iter().map(parse_rule).collect::<Result<_, _>>()?,
            Some(_) => return Err("`rules` is not an array".to_owned()),
            None => Vec::new(),
        };
        Ok(Self {
            start,
            rules,
            seen: Vec::new(),
            next_id: 1,
            log_answers: value.get("log_answers") == Some(&Value::Bool(true)),
            log_documents: value.get("log_documents") == Some(&Value::Bool(true)),
            log_positions: value.get("log_positions") == Some(&Value::Bool(true)),
            utf8: value.get("encoding").and_then(Value::as_str) == Some("utf-8"),
            documents: std::collections::HashMap::new(),
        })
    }

    /// What the server does as it starts.
    pub fn start(&mut self) -> Vec<Action> {
        let steps = self.start.clone();
        self.run(&steps, None)
    }

    /// What the server does for `message`, a message it read.
    pub fn answer(&mut self, message: &Value) -> Vec<Action> {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").filter(|_| method.is_some()).cloned();
        let Some(method) = method else {
            // An answer to the server's own request.
            if self.log_answers {
                return vec![Action::Err(format!("answer: {message}\n").into_bytes())];
            }
            return Vec::new();
        };
        let mut kept = self.keep(method, message);
        if self.log_positions {
            kept.extend(asked(method, message));
        }
        let nth = self.count(method);
        let steps = self
            .rules
            .iter()
            .find(|rule| rule.method == method && rule.nth.is_none_or(|n| n == nth))
            .map(|rule| rule.steps.clone());
        let mut actions = match steps {
            Some(steps) => self.run(&steps, id.as_ref()),
            None if method == "exit" => vec![Action::Exit(0)],
            None => match id {
                Some(id) => vec![Action::Out(frame(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": format!("no rule for {method}")},
                })))],
                None => Vec::new(),
            },
        };
        kept.append(&mut actions);
        kept
    }

    /// Keeps the document a sync notification names as it says, if documents are kept, and
    /// what to write of it.
    fn keep(&mut self, method: &str, message: &Value) -> Vec<Action> {
        if !self.log_documents {
            return Vec::new();
        }
        let params = &message["params"];
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let version = params["textDocument"]["version"].as_i64().unwrap_or(0);
        match method {
            "textDocument/didOpen" => {
                let text = params["textDocument"]["text"].as_str().unwrap_or_default();
                self.documents.insert(uri.clone(), text.to_owned());
            }
            "textDocument/didChange" => {
                let utf8 = self.utf8;
                let Some(text) = self.documents.get_mut(&uri) else {
                    return vec![Action::Err(
                        format!("change of no document {uri}\n").into_bytes(),
                    )];
                };
                for change in params["contentChanges"].as_array().into_iter().flatten() {
                    apply(text, change, utf8);
                }
            }
            "textDocument/didClose" => {
                self.documents.remove(&uri);
                return vec![Action::Err(format!("closed {uri}\n").into_bytes())];
            }
            _ => return Vec::new(),
        }
        let text = &self.documents[&uri];
        vec![Action::Err(
            format!(
                "doc {uri} v{version} len={} hash={:016x}\n",
                text.len(),
                fnv(text.as_bytes())
            )
            .into_bytes(),
        )]
    }

    /// Counts a message of `method` and gives its number, from 1.
    fn count(&mut self, method: &str) -> usize {
        if let Some((_, count)) = self.seen.iter_mut().find(|(seen, _)| seen == method) {
            *count += 1;
            return *count;
        }
        self.seen.push((method.to_owned(), 1));
        1
    }

    fn run(&mut self, steps: &[Step], id: Option<&Value>) -> Vec<Action> {
        let mut actions = Vec::new();
        for step in steps {
            match step {
                Step::Reply(result) => actions.push(reply(id, "result", result)),
                Step::Error(error) => actions.push(reply(id, "error", error)),
                Step::Notify(message) => {
                    actions.push(Action::Out(frame(&message_with(message, None))))
                }
                Step::Request(message) => {
                    let id = self.next_id;
                    self.next_id += 1;
                    actions.push(Action::Out(frame(&message_with(message, Some(id)))));
                }
                Step::Raw(text) => actions.push(Action::Out(text.as_bytes().to_vec())),
                Step::Oversized(len) => actions.push(Action::Out(oversized(*len))),
                Step::Flood { count, method } => {
                    let one = frame(&json!({"jsonrpc": "2.0", "method": method, "params": {}}));
                    actions.push(Action::Out(one.repeat(*count)));
                }
                Step::Hang => {}
                Step::Sleep(ms) => actions.push(Action::Sleep(*ms)),
                Step::Stderr { text, repeat } => {
                    actions.push(Action::Err(text.repeat(*repeat).into_bytes()))
                }
                Step::Exit(code) => actions.push(Action::Exit(*code)),
            }
        }
        actions
    }
}

fn parse_rule(rule: &Value) -> Result<Rule, String> {
    let method = rule
        .get("method")
        .and_then(Value::as_str)
        .ok_or("a rule has no `method`")?
        .to_owned();
    let nth = match rule.get("nth") {
        Some(nth) => Some(
            nth.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n > 0)
                .ok_or("`nth` is not a count from 1")?,
        ),
        None => None,
    };
    let steps = parse_steps(rule.get("do").ok_or("a rule has no `do`")?)?;
    Ok(Rule { method, nth, steps })
}

fn parse_steps(steps: &Value) -> Result<Vec<Step>, String> {
    steps
        .as_array()
        .ok_or("`do` and `start` are arrays")?
        .iter()
        .map(parse_step)
        .collect()
}

fn parse_step(step: &Value) -> Result<Step, String> {
    let object = step
        .as_object()
        .filter(|object| object.len() == 1)
        .ok_or("an action is an object of one key")?;
    let (name, value) = object.iter().next().ok_or("an empty action")?;
    let count = |value: &Value| value.as_u64().and_then(|n| usize::try_from(n).ok());
    Ok(match name.as_str() {
        "reply" => Step::Reply(value.clone()),
        "error" => Step::Error(value.clone()),
        "notify" => Step::Notify(method_and_params(value)?),
        "request" => Step::Request(method_and_params(value)?),
        "raw" => Step::Raw(value.as_str().ok_or("`raw` is a string")?.to_owned()),
        "oversized" => Step::Oversized(count(value).ok_or("`oversized` is a count")?),
        "flood" => Step::Flood {
            count: value
                .get("count")
                .and_then(count)
                .ok_or("`flood` has no `count`")?,
            method: value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("$/flood")
                .to_owned(),
        },
        "hang" => Step::Hang,
        "sleep_ms" => Step::Sleep(value.as_u64().ok_or("`sleep_ms` is a count")?),
        "stderr" => Step::Stderr {
            text: value
                .get("text")
                .and_then(Value::as_str)
                .ok_or("`stderr` has no `text`")?
                .to_owned(),
            repeat: value
                .get("repeat")
                .map_or(Some(1), count)
                .ok_or("`repeat` is a count")?,
        },
        "exit" => Step::Exit(
            value
                .as_i64()
                .and_then(|code| i32::try_from(code).ok())
                .ok_or("`exit` is a code")?,
        ),
        other => return Err(format!("no action `{other}`")),
    })
}

fn method_and_params(value: &Value) -> Result<Value, String> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .ok_or("a message has no `method`")?;
    let mut message = Map::new();
    message.insert("method".to_owned(), json!(method));
    if let Some(params) = value.get("params") {
        message.insert("params".to_owned(), params.clone());
    }
    Ok(Value::Object(message))
}

fn message_with(message: &Value, id: Option<u64>) -> Value {
    let mut message = message.clone();
    if let Value::Object(map) = &mut message {
        map.insert("jsonrpc".to_owned(), json!("2.0"));
        if let Some(id) = id {
            map.insert("id".to_owned(), json!(id));
        }
    }
    message
}

fn reply(id: Option<&Value>, key: &str, value: &Value) -> Action {
    let mut message = Map::new();
    message.insert("jsonrpc".to_owned(), json!("2.0"));
    message.insert("id".to_owned(), id.cloned().unwrap_or(Value::Null));
    message.insert(key.to_owned(), value.clone());
    Action::Out(frame(&Value::Object(message)))
}

fn frame(message: &Value) -> Vec<u8> {
    oso_lsp::framing::encode(message.to_string().as_bytes())
}

/// A message of `len` bytes that is JSON when it is at least 4 long: spaces, then `null`.
fn oversized(len: usize) -> Vec<u8> {
    let mut body = vec![b' '; len.saturating_sub(4)];
    body.extend(&b"null"[..len.min(4)]);
    oso_lsp::framing::encode(&body)
}

/// Applies one content change to `text`, finding its positions by a scan of the fake's own:
/// lines end at `\n`, and a column counts bytes, or UTF-16 units unless `utf8`.
fn apply(text: &mut String, change: &Value, utf8: bool) {
    let new = change["text"].as_str().unwrap_or_default();
    let Some(range) = change.get("range") else {
        *text = new.to_owned();
        return;
    };
    let at = |position: &Value| {
        let line = position["line"].as_u64().unwrap_or(0) as usize;
        let column = position["character"].as_u64().unwrap_or(0) as usize;
        let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let start = start.min(text.len());
        let mut units = 0;
        for (index, c) in text[start..].char_indices() {
            if units >= column || c == '\n' {
                return start + index;
            }
            units += if utf8 { c.len_utf8() } else { c.len_utf16() };
        }
        text.len()
    };
    let (start, end) = (at(&range["start"]), at(&range["end"]));
    text.replace_range(start..end.max(start), new);
}

/// FNV-1a, 64 bits, which the tests compute too.
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What a request with a position, or a cancel, writes when positions are logged.
fn asked(method: &str, message: &Value) -> Option<Action> {
    let params = &message["params"];
    let line = if method == "$/cancelRequest" {
        format!("cancel {}\n", params["id"])
    } else {
        let position = params.get("position")?;
        format!(
            "asked {method} {}:{}\n",
            position["line"], position["character"]
        )
    };
    Some(Action::Err(line.into_bytes()))
}

#[cfg(test)]
mod tests {
    use oso_lsp::framing::{Frame, Reader};
    use serde_json::{Value, json};

    use super::{Action, Script};

    /// The messages in what `actions` write to the standard output.
    fn written(actions: &[Action]) -> Vec<Value> {
        let bytes: Vec<u8> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Out(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let mut reader = Reader::new(bytes.as_slice());
        let mut messages = Vec::new();
        while let Ok(Some(Frame::Message(body))) = reader.next_frame() {
            messages.push(serde_json::from_slice(&body).unwrap_or(Value::Null));
        }
        messages
    }

    fn request(id: u64, method: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {}})
    }

    #[test]
    fn requests_get_their_rules_reply() {
        let mut script = Script::parse(
            r#"{"rules": [{"method": "initialize", "do": [{"reply": {"capabilities": {}}}]}]}"#,
        )
        .unwrap();
        let actions = script.answer(&request(7, "initialize"));
        assert_eq!(
            written(&actions),
            [json!({"jsonrpc": "2.0", "id": 7, "result": {"capabilities": {}}})]
        );
    }

    #[test]
    fn a_request_with_no_rule_is_not_found_and_exit_ends() {
        let mut script = Script::parse("{}").unwrap();
        let actions = script.answer(&request(1, "textDocument/hover"));
        assert_eq!(written(&actions)[0]["error"]["code"], json!(-32601));
        assert!(
            script
                .answer(&json!({"jsonrpc": "2.0", "method": "initialized"}))
                .is_empty()
        );
        assert_eq!(
            script.answer(&json!({"jsonrpc": "2.0", "method": "exit"})),
            [Action::Exit(0)]
        );
        // An answer to the server's own request.
        assert!(
            script
                .answer(&json!({"jsonrpc": "2.0", "id": 1, "result": null}))
                .is_empty()
        );
    }

    #[test]
    fn nth_matches_only_that_message() {
        let mut script = Script::parse(
            r#"{"rules": [
                {"method": "textDocument/hover", "nth": 2, "do": [{"hang": true}]},
                {"method": "textDocument/hover", "do": [{"reply": null}]}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            written(&script.answer(&request(1, "textDocument/hover"))).len(),
            1
        );
        assert!(script.answer(&request(2, "textDocument/hover")).is_empty());
        assert_eq!(
            written(&script.answer(&request(3, "textDocument/hover"))).len(),
            1
        );
    }

    #[test]
    fn misbehaviour_comes_on_cue() {
        let mut script = Script::parse(
            r#"{"start": [{"request": {"method": "workspace/configuration", "params": {"items": []}}}],
                "rules": [{"method": "shutdown", "do": [
                    {"raw": "Content-Length: x\r\n\r\n"},
                    {"oversized": 20},
                    {"flood": {"count": 3}},
                    {"stderr": {"text": "ab", "repeat": 2}},
                    {"sleep_ms": 5},
                    {"error": {"code": 1, "message": "no"}},
                    {"exit": 3}
                ]}]}"#,
        )
        .unwrap();
        let start = written(&script.start());
        assert_eq!(start[0]["id"], json!(1));
        assert_eq!(start[0]["method"], json!("workspace/configuration"));
        let actions = script.answer(&request(9, "shutdown"));
        assert_eq!(
            actions[0],
            Action::Out(b"Content-Length: x\r\n\r\n".to_vec())
        );
        let oversized = format!("Content-Length: 20\r\n\r\n{}null", " ".repeat(16));
        assert_eq!(actions[1], Action::Out(oversized.into_bytes()));
        assert_eq!(written(&actions[2..3]).len(), 3);
        assert_eq!(actions[3], Action::Err(b"abab".to_vec()));
        assert_eq!(actions[4], Action::Sleep(5));
        assert_eq!(written(&actions[5..6])[0]["error"]["message"], json!("no"));
        assert_eq!(actions[6], Action::Exit(3));
    }

    #[test]
    fn bad_scripts_say_why() {
        for bad in [
            "[",
            r#"{"rules": {}}"#,
            r#"{"rules": [{"do": []}]}"#,
            r#"{"rules": [{"method": "a"}]}"#,
            r#"{"rules": [{"method": "a", "nth": 0, "do": []}]}"#,
            r#"{"rules": [{"method": "a", "do": [{"dance": 1}]}]}"#,
            r#"{"rules": [{"method": "a", "do": [{"reply": 1, "error": 2}]}]}"#,
        ] {
            assert!(Script::parse(bad).is_err(), "{bad}");
        }
    }
}
