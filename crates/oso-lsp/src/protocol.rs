//! The messages of a server's life, as plain Rust types (ADR 0138, step 3): what oso sends to
//! start and stop a server, how a message from it is told apart (an answer, a request or a
//! notification), what oso answers each of its requests, and what it reads from progress and
//! messages for the person. The capabilities claimed are only the ones step 3 uses (Q5);
//! each later step adds its own.
//!
//! Must not depend on: `server`, threads or processes; this builds and reads values, and the
//! session thread sends and receives them.

mod located;
mod published;
#[cfg(test)]
mod tests;

pub use located::{Located, definition_params, located, references_params};
pub use published::{Published, Reported, published};
use serde_json::{Map, Value, json};

use crate::json::{integer_in, string, uinteger};
use crate::limits::{
    CONFIGURATION_ITEMS_MAX, ID_BYTES_MAX, KEYWORD_BYTES_MAX, MESSAGE_BYTES_MAX, METHOD_BYTES_MAX,
    SHOWN_BYTES_MAX, URI_BYTES_MAX,
};
use crate::position::Encoding;

/// JSON-RPC's code for a method the receiver does not have.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC's code for parameters the receiver cannot read.
pub const INVALID_PARAMS: i64 = -32602;

/// A message from a server, told apart by its fields.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// An answer to one of oso's requests, whose ids are integers.
    Answer { id: i64, outcome: Outcome },
    /// An answer whose id is no integer oso could have sent.
    Stray,
    /// A request oso must answer.
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    /// A request whose id oso does not answer: a string id longer than [`ID_BYTES_MAX`].
    Unanswerable { method: String },
    /// A notification.
    Notification { method: String, params: Value },
}

/// What an answer carried.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Result(Value),
    Error { code: i64, message: String },
}

/// A server's id for its request, given back in the answer as it came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestId {
    Number(i64),
    Text(String),
}

impl RequestId {
    fn to_value(&self) -> Value {
        match self {
            Self::Number(number) => json!(number),
            Self::Text(text) => json!(text),
        }
    }
}

/// A message that is not JSON-RPC as oso reads it: the server is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotRpc;

impl std::fmt::Display for NotRpc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a message that is not JSON-RPC")
    }
}

/// Tells `message` apart. An object with a `method` is a request when it has an `id` and a
/// notification otherwise; one with an `id` and a `result` or `error` is an answer. Anything
/// else, a method's name over [`METHOD_BYTES_MAX`] included, is [`NotRpc`].
///
/// # Errors
///
/// [`NotRpc`], as above.
pub fn classify(message: Value) -> Result<Incoming, NotRpc> {
    let Value::Object(mut fields) = message else {
        return Err(NotRpc);
    };
    let params = fields.remove("params").unwrap_or(Value::Null);
    if let Some(method) = fields.get("method") {
        let method = string_within(method, METHOD_BYTES_MAX).ok_or(NotRpc)?;
        return Ok(match fields.get("id") {
            None => Incoming::Notification { method, params },
            Some(Value::Number(_)) => match fields.get("id").and_then(Value::as_i64) {
                Some(id) => Incoming::Request {
                    id: RequestId::Number(id),
                    method,
                    params,
                },
                None => Incoming::Unanswerable { method },
            },
            Some(Value::String(id)) if id.len() <= ID_BYTES_MAX => Incoming::Request {
                id: RequestId::Text(id.clone()),
                method,
                params,
            },
            Some(Value::String(_)) => Incoming::Unanswerable { method },
            Some(_) => return Err(NotRpc),
        });
    }
    let id = fields.get("id").ok_or(NotRpc)?.as_i64();
    let outcome = if let Some(error) = fields.get("error") {
        Outcome::Error {
            code: error
                .get("code")
                .and_then(|code| integer_in(code, i64::MIN..=i64::MAX))
                .unwrap_or(0),
            message: error
                .get("message")
                .and_then(|message| string(message, SHOWN_BYTES_MAX))
                .unwrap_or("")
                .to_owned(),
        }
    } else if let Some(result) = fields.remove("result") {
        Outcome::Result(result)
    } else {
        return Err(NotRpc);
    };
    Ok(match id {
        Some(id) => Incoming::Answer { id, outcome },
        None => Incoming::Stray,
    })
}

/// `value` as a string of at most `max` bytes, `None` if it is longer or no string.
fn string_within(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .filter(|text| text.len() <= max)
        .map(str::to_owned)
}

/// A request's body.
pub fn request(id: i64, method: &str, params: Value) -> Vec<u8> {
    body(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
}

/// A notification's body.
pub fn notification(method: &str, params: Value) -> Vec<u8> {
    body(json!({"jsonrpc": "2.0", "method": method, "params": params}))
}

/// The body of an answer to the server's request `id`.
pub fn answer(id: &RequestId, outcome: Result<Value, (i64, &str)>) -> Vec<u8> {
    body(match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id.to_value(), "result": result}),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id.to_value(),
            "error": {"code": code, "message": message},
        }),
    })
}

fn body(value: Value) -> Vec<u8> {
    // A `Value` always serializes: its keys are strings.
    serde_json::to_vec(&value).unwrap_or_default()
}

/// What oso tells a server as it starts it.
#[derive(Debug, Clone, PartialEq)]
pub struct Initialize<'a> {
    /// The project's root as a URI, the only workspace folder.
    pub root_uri: &'a str,
    /// The folder's name, the root's last component.
    pub root_name: &'a str,
    /// `initializationOptions`, the entry's settings, if it has any.
    pub options: Option<&'a Value>,
}

/// `initialize`'s parameters: oso's process, the root as the one workspace folder, the entry's
/// settings, and the capabilities the steps so far use: positions in UTF-8 or UTF-16,
/// progress, `workspace/configuration`, saves (step 4), diagnostics with their versions and
/// tags (step 6), and definitions, with links, and references (step 8).
pub fn initialize_params(start: &Initialize<'_>) -> Value {
    let mut params = json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "oso", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": start.root_uri,
        "workspaceFolders": [{"uri": start.root_uri, "name": start.root_name}],
        "capabilities": {
            "general": {"positionEncodings": Encoding::OFFERED},
            "window": {"workDoneProgress": true},
            "workspace": {"configuration": true},
            "textDocument": {
                "synchronization": {"didSave": true},
                "publishDiagnostics": {
                    "versionSupport": true,
                    "tagSupport": {"valueSet": [1, 2]},
                },
                "definition": {"linkSupport": true},
                "references": {},
            },
        },
    });
    if let (Some(options), Value::Object(fields)) = (start.options, &mut params) {
        fields.insert("initializationOptions".into(), options.clone());
    }
    params
}

/// The encoding the server chose in `initialize`'s result, UTF-16 when it says nothing;
/// `None` for one oso did not offer, which stops it.
pub fn chosen_encoding(result: &Value) -> Option<Encoding> {
    let chosen = result.get("capabilities")?.get("positionEncoding");
    Encoding::chosen(chosen.and_then(|value| string(value, KEYWORD_BYTES_MAX)))
}

/// How a server wants documents synced, as its `textDocumentSync` says (step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sync {
    /// It is told of opens and closes.
    pub open_close: bool,
    /// How it is told of changes.
    pub change: SyncKind,
    /// It is told of saves.
    pub save: bool,
}

/// How a server is told of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    /// Not at all.
    None,
    /// The whole text each time.
    Full,
    /// The ranges that changed.
    Incremental,
}

/// The sync `initialize`'s result asks for. A bare kind, the protocol's older form, also asks
/// for opens and closes, not saves; nothing asks for none. A server that wants a save's text
/// (`includeText`) is told of no save: oso sends none.
pub fn sync(result: &Value) -> Sync {
    let kind = |value: Option<&Value>| match value.and_then(|v| integer_in(v, 0..=2)) {
        Some(1) => SyncKind::Full,
        Some(2) => SyncKind::Incremental,
        _ => SyncKind::None,
    };
    let given = result
        .get("capabilities")
        .and_then(|c| c.get("textDocumentSync"));
    match given {
        Some(Value::Object(options)) => Sync {
            open_close: options.get("openClose") == Some(&Value::Bool(true)),
            change: kind(options.get("change")),
            save: match options.get("save") {
                Some(Value::Bool(on)) => *on,
                Some(Value::Object(save)) => save.get("includeText") != Some(&Value::Bool(true)),
                _ => false,
            },
        },
        Some(number @ Value::Number(_)) => {
            let change = kind(Some(number));
            Sync {
                open_close: change != SyncKind::None,
                change,
                save: false,
            }
        }
        _ => Sync {
            open_close: false,
            change: SyncKind::None,
            save: false,
        },
    }
}

/// The name the server gives itself, cut, for `*LSP*`.
pub fn server_name(result: &Value) -> Option<&str> {
    let info = result.get("serverInfo")?;
    string(info.get("name")?, SHOWN_BYTES_MAX)
}

/// What oso answers the server's request `method`: `workspace/configuration` from `options`,
/// the entry's settings under `section`; an empty success for progress tokens and
/// registrations; `applied: false`, `success: false` and `null` for what oso never lets a
/// server do by itself (ADR 0138, Bounds); and [`METHOD_NOT_FOUND`] for anything else.
///
/// # Errors
///
/// The code and message of the error answer.
pub fn answer_for(
    method: &str,
    params: &Value,
    section: Option<&str>,
    options: &Value,
) -> Result<Value, (i64, &'static str)> {
    match method {
        "workspace/configuration" => configuration(params, section, options),
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability" => Ok(Value::Null),
        "workspace/applyEdit" => Ok(json!({"applied": false})),
        "window/showDocument" => Ok(json!({"success": false})),
        "window/showMessageRequest" => Ok(Value::Null),
        _ => Err((METHOD_NOT_FOUND, "oso does not have this method")),
    }
}

/// `workspace/configuration`'s answer: for each item, the part of `options` its section names
/// below `section`, the entry's own (`rust-analyzer.cargo` is `options`' `cargo`), and
/// `null` for any other.
fn configuration(
    params: &Value,
    section: Option<&str>,
    options: &Value,
) -> Result<Value, (i64, &'static str)> {
    let items = params
        .get("items")
        .and_then(Value::as_array)
        .ok_or((INVALID_PARAMS, "no items"))?;
    if items.len() > CONFIGURATION_ITEMS_MAX {
        return Err((INVALID_PARAMS, "too many items"));
    }
    // Each item's answer is at most the whole settings: past the bound of a message, no answer
    // is built at all.
    let whole = serde_json::to_vec(options).map_or(0, |bytes| bytes.len());
    if items.len().saturating_mul(whole) > MESSAGE_BYTES_MAX {
        return Err((INVALID_PARAMS, "the answer would be too long"));
    }
    let answers = items
        .iter()
        .map(|item| {
            let asked = item
                .get("section")
                .and_then(|asked| string(asked, URI_BYTES_MAX));
            match (asked, section) {
                (Some(asked), Some(section)) => setting(options, asked, section),
                _ => Value::Null,
            }
        })
        .collect();
    Ok(Value::Array(answers))
}

/// The part of `options` that `asked` names, when it is `section` or below it.
fn setting(options: &Value, asked: &str, section: &str) -> Value {
    let Some(rest) = asked.strip_prefix(section) else {
        return Value::Null;
    };
    let mut at = options;
    if !rest.is_empty() {
        let Some(path) = rest.strip_prefix('.') else {
            return Value::Null;
        };
        for key in path.split('.') {
            match at.get(key) {
                Some(inner) => at = inner,
                None => return Value::Null,
            }
        }
    }
    at.clone()
}

/// A progress token, as the server named it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    Number(i64),
    Text(String),
}

/// What a `$/progress` notification says of its token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Work began, or went on, done to this percentage if it says.
    Working(Option<u8>),
    /// The work ended.
    Ended,
}

/// The token and what a `$/progress` notification's `params` say, `None` if it is not work
/// done progress oso reads.
pub fn progress(params: &Value) -> Option<(Token, Progress)> {
    let token = match params.get("token")? {
        Value::Number(number) => Token::Number(number.as_i64()?),
        Value::String(text) if text.len() <= ID_BYTES_MAX => Token::Text(text.clone()),
        _ => return None,
    };
    let value = params.get("value")?;
    let percentage = value
        .get("percentage")
        .and_then(uinteger)
        .map(|percent| u8::try_from(percent.min(100)).unwrap_or(100));
    let progress = match string(value.get("kind")?, KEYWORD_BYTES_MAX)? {
        "begin" | "report" => Progress::Working(percentage),
        "end" => Progress::Ended,
        _ => return None,
    };
    Some((token, progress))
}

/// How serious a server's message is, as `window/showMessage` and `window/logMessage` say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Log,
}

/// The severity and text of a `window/showMessage` or `window/logMessage`, cut to
/// [`SHOWN_BYTES_MAX`]; `None` if it has no text.
pub fn message(params: &Value) -> Option<(Severity, &str)> {
    let severity = match params.get("type").and_then(|kind| integer_in(kind, 1..=4)) {
        Some(1) => Severity::Error,
        Some(2) => Severity::Warning,
        Some(3) => Severity::Info,
        _ => Severity::Log,
    };
    Some((severity, string(params.get("message")?, SHOWN_BYTES_MAX)?))
}

/// The entries of `options` merged key by key with `over`'s, `over` winning: objects merge,
/// any other value replaces. A built-in entry's settings and the person's (ADR 0138, Q3).
pub fn merge(options: &mut Value, over: &Value) {
    let (Value::Object(base), Value::Object(over)) = (&mut *options, over) else {
        *options = over.clone();
        return;
    };
    merge_maps(base, over);
}

fn merge_maps(base: &mut Map<String, Value>, over: &Map<String, Value>) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(Value::Object(inner)), Value::Object(over)) => merge_maps(inner, over),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}
