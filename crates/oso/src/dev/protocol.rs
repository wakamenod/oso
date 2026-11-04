//! The control socket's JSON Lines protocol (ADR 0023).
//!
//! Must not depend on: sockets, threads or editor state; it only parses and
//! formats messages.
//!
//! - Request: `{"id": 1, "method": "send-keys", "params": {"keys": "C-x"}}`.
//!   `id` is optional and may be a number, a string or null. `params` is
//!   optional.
//! - Response: `{"id": 1, "result": ...}` or
//!   `{"id": 1, "error": {"message": "..."}}`, echoing the request's `id`.

use oso_core::key::{KeyPress, KeySequence};
use serde_json::{Map, Value, json};

/// The protocol version reported by `version`. Bump it on incompatible
/// changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest request or response line, without its newline.
pub const MAX_LINE: usize = 1 << 20;

/// The most keys one `send-keys` request may send.
pub const MAX_KEYS: usize = 4096;

/// The longest string `id`, in bytes. Ids are echoed, so this keeps
/// responses small.
pub const MAX_ID: usize = 256;

/// The most characters of a method name echoed in an error.
const MAX_ECHO: usize = 64;

/// The longest command name `run-command` accepts, in bytes. Names are
/// echoed in errors, so this keeps responses small.
const MAX_COMMAND_NAME: usize = 256;

/// A method and its parsed parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// The protocol version and build information.
    Version,
    /// Key presses to handle, in order.
    SendKeys(Vec<KeyPress>),
    /// A PNG of the current frame.
    Screenshot,
    /// The editor state, with the selected window's visible text.
    GetState,
    /// Whole rows of the selected window's buffer.
    GetText { start_row: usize, rows: usize },
    /// Run a registered command, with a numeric argument if it takes one.
    RunCommand { name: String, arg: Option<isize> },
    /// Show a buffer in the selected window.
    ShowBuffer(u64),
}

/// A parsed request.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Echoed in the response.
    pub id: Value,
    pub method: Method,
}

/// A request that cannot be handled: the `id` to answer with, if one was
/// found, and the error message.
#[derive(Debug, Clone, PartialEq)]
pub struct BadRequest {
    pub id: Value,
    pub message: String,
}

/// Parses one request line.
pub fn parse(line: &str) -> Result<Request, BadRequest> {
    let bad = |id: &Value, message: String| BadRequest {
        id: id.clone(),
        message,
    };
    let value: Value =
        serde_json::from_str(line).map_err(|e| bad(&Value::Null, format!("invalid JSON: {e}")))?;
    let Value::Object(request) = value else {
        return Err(bad(&Value::Null, "the request is not a JSON object".into()));
    };
    let id = match request.get("id") {
        None => Value::Null,
        Some(Value::String(id)) if id.len() > MAX_ID => {
            return Err(bad(
                &Value::Null,
                format!("id is longer than {MAX_ID} bytes"),
            ));
        }
        Some(id @ (Value::Null | Value::Number(_) | Value::String(_))) => id.clone(),
        Some(_) => {
            return Err(bad(
                &Value::Null,
                "id must be a number, a string or null".into(),
            ));
        }
    };
    let Some(Value::String(method)) = request.get("method") else {
        return Err(bad(&id, "method must be a string".into()));
    };
    let empty = Map::new();
    let params = match request.get("params") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(params)) => params,
        Some(_) => return Err(bad(&id, "params must be an object".into())),
    };
    let method = match method.as_str() {
        "version" => Method::Version,
        "screenshot" => Method::Screenshot,
        "get-state" => Method::GetState,
        "send-keys" => Method::SendKeys(parse_keys(params).map_err(|m| bad(&id, m))?),
        "run-command" => parse_run_command(params).map_err(|m| bad(&id, m))?,
        "get-text" => Method::GetText {
            start_row: parse_size(params, "get-text", "start_row").map_err(|m| bad(&id, m))?,
            rows: parse_size(params, "get-text", "rows").map_err(|m| bad(&id, m))?,
        },
        "show-buffer" => Method::ShowBuffer(
            params
                .get("buffer")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    bad(
                        &id,
                        "show-buffer needs a numeric parameter \"buffer\"".into(),
                    )
                })?,
        ),
        other => {
            let mut shown: String = other.chars().take(MAX_ECHO).collect();
            if shown.len() < other.len() {
                shown.push('…');
            }
            return Err(bad(&id, format!("unknown method {shown:?}")));
        }
    };
    Ok(Request { id, method })
}

/// The `keys` parameter of `send-keys`, in Emacs notation.
fn parse_keys(params: &Map<String, Value>) -> Result<Vec<KeyPress>, String> {
    let Some(Value::String(keys)) = params.get("keys") else {
        return Err("send-keys needs a string parameter \"keys\"".into());
    };
    let KeySequence(keys) = keys.parse().map_err(|e| format!("{e}"))?;
    if keys.is_empty() {
        return Err("send-keys needs at least one key".into());
    }
    if keys.len() > MAX_KEYS {
        return Err(format!("send-keys takes at most {MAX_KEYS} keys"));
    }
    Ok(keys)
}

/// The parameters of `run-command`: a string `name` and an optional
/// integer `arg`.
fn parse_run_command(params: &Map<String, Value>) -> Result<Method, String> {
    let Some(Value::String(name)) = params.get("name") else {
        return Err("run-command needs a string parameter \"name\"".into());
    };
    if name.len() > MAX_COMMAND_NAME {
        return Err(format!(
            "run-command takes names of at most {MAX_COMMAND_NAME} bytes"
        ));
    }
    let arg = match params.get("arg") {
        None | Some(Value::Null) => None,
        Some(arg) => Some(
            arg.as_i64()
                .and_then(|arg| isize::try_from(arg).ok())
                .ok_or("run-command's \"arg\" must be an integer")?,
        ),
    };
    Ok(Method::RunCommand {
        name: name.clone(),
        arg,
    })
}

/// A required parameter that is a non-negative integer.
fn parse_size(params: &Map<String, Value>, method: &str, name: &str) -> Result<usize, String> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{method} needs a non-negative integer parameter {name:?}"))
}

/// A success response line, without the newline.
pub fn success(id: &Value, result: Value) -> String {
    json!({ "id": id, "result": result }).to_string()
}

/// An error response line, without the newline.
pub fn failure(id: &Value, message: &str) -> String {
    json!({ "id": id, "error": { "message": message } }).to_string()
}

/// Builds a request line, without the newline. For `oso ctl` and tests.
pub fn request(id: u64, method: &str, params: Option<Value>) -> String {
    let mut request = json!({ "id": id, "method": method });
    if let Some(params) = params {
        request["params"] = params;
    }
    request.to_string()
}

#[cfg(test)]
mod tests {
    use super::{MAX_ID, MAX_KEYS, MAX_LINE, Method, Request, failure, parse, request, success};
    use serde_json::{Value, json};

    fn parsed(line: &str) -> Request {
        parse(line).unwrap_or_else(|e| panic!("{}", e.message))
    }

    fn error(line: &str) -> (Value, String) {
        let e = parse(line).unwrap_err();
        (e.id, e.message)
    }

    #[test]
    fn parses_each_method() {
        assert_eq!(
            parsed(r#"{"id": 1, "method": "version"}"#).method,
            Method::Version
        );
        assert_eq!(
            parsed(r#"{"id": 2, "method": "screenshot", "params": {}}"#).method,
            Method::Screenshot
        );
        assert_eq!(
            parsed(r#"{"id": 3, "method": "get-state", "params": null}"#).method,
            Method::GetState
        );
        let keys = parsed(r#"{"id": 4, "method": "send-keys", "params": {"keys": "C-x <right>"}}"#);
        assert_eq!(
            keys.method,
            Method::SendKeys(vec!["C-x".parse().unwrap(), "<right>".parse().unwrap()])
        );
    }

    #[test]
    fn echoes_any_scalar_id() {
        assert_eq!(parsed(r#"{"id": "a", "method": "version"}"#).id, json!("a"));
        assert_eq!(
            parsed(r#"{"id": null, "method": "version"}"#).id,
            json!(null)
        );
        assert_eq!(parsed(r#"{"method": "version"}"#).id, json!(null));
        assert_eq!(parsed(r#"{"id": 1.5, "method": "version"}"#).id, json!(1.5));
    }

    #[test]
    fn rejects_malformed_requests() {
        assert!(error("not json").1.starts_with("invalid JSON"));
        assert_eq!(error("[1]").1, "the request is not a JSON object");
        assert_eq!(
            error(r#"{"id": [1], "method": "version"}"#),
            (json!(null), "id must be a number, a string or null".into())
        );
        assert_eq!(
            error(r#"{"id": 7}"#),
            (json!(7), "method must be a string".into())
        );
        assert_eq!(
            error(r#"{"id": 7, "method": "version", "params": [1]}"#),
            (json!(7), "params must be an object".into())
        );
        assert_eq!(
            error(r#"{"id": 7, "method": "no-such-method"}"#),
            (json!(7), "unknown method \"no-such-method\"".into())
        );
    }

    #[test]
    fn errors_stay_short_whatever_the_request() {
        // JSON escapes for U+0001, which errors print as `\u{1}`.
        let long = "\\u0001".repeat(MAX_LINE / 8);
        let method = error(&format!(r#"{{"id": 1, "method": "{long}"}}"#)).1;
        assert!(method.starts_with("unknown method"), "{method}");
        assert!(method.len() < 600, "{} bytes", method.len());
        let keys = error(&format!(
            r#"{{"id": 1, "method": "send-keys", "params": {{"keys": "{long}"}}}}"#
        ))
        .1;
        assert!(keys.starts_with("invalid key"), "{keys}");
        assert!(keys.len() < 400, "{} bytes", keys.len());
        let id = "a".repeat(MAX_ID + 1);
        assert_eq!(
            error(&format!(r#"{{"id": "{id}", "method": "version"}}"#)),
            (json!(null), format!("id is longer than {MAX_ID} bytes"))
        );
        let id = "a".repeat(MAX_ID);
        assert!(parse(&format!(r#"{{"id": "{id}", "method": "version"}}"#)).is_ok());
    }

    #[test]
    fn rejects_bad_keys() {
        let line =
            |params: &str| format!(r#"{{"id": 1, "method": "send-keys", "params": {params}}}"#);
        assert!(error(&line("{}")).1.contains("\"keys\""));
        assert!(error(&line(r#"{"keys": 1}"#)).1.contains("\"keys\""));
        assert_eq!(
            error(&line(r#"{"keys": " "}"#)).1,
            "send-keys needs at least one key"
        );
        assert!(error(&line(r#"{"keys": "C-foo"}"#)).1.contains("C-foo"));
        let many = vec!["a"; MAX_KEYS + 1].join(" ");
        assert!(
            error(&line(&format!(r#"{{"keys": "{many}"}}"#)))
                .1
                .contains("at most")
        );
        let most = vec!["a"; MAX_KEYS].join(" ");
        assert!(parse(&line(&format!(r#"{{"keys": "{most}"}}"#))).is_ok());
    }

    #[test]
    fn parses_run_command() {
        let line = |params: &str| format!(r#"{{"method": "run-command", "params": {params}}}"#);
        assert_eq!(
            parsed(&line(r#"{"name": "forward-char"}"#)).method,
            Method::RunCommand {
                name: "forward-char".into(),
                arg: None
            }
        );
        assert_eq!(
            parsed(&line(r#"{"name": "next-line", "arg": -3}"#)).method,
            Method::RunCommand {
                name: "next-line".into(),
                arg: Some(-3)
            }
        );
        assert!(error(&line("{}")).1.contains("\"name\""));
        assert!(error(&line(r#"{"name": 1}"#)).1.contains("\"name\""));
        assert!(
            error(&line(r#"{"name": "x", "arg": "2"}"#))
                .1
                .contains("integer")
        );
        assert!(
            error(&line(r#"{"name": "x", "arg": 1.5}"#))
                .1
                .contains("integer")
        );
        let long = "a".repeat(MAX_LINE / 2);
        let long = error(&line(&format!(r#"{{"name": "{long}"}}"#))).1;
        assert!(long.len() < 100, "{long}");
    }

    #[test]
    fn parses_get_text_and_show_buffer() {
        let line =
            |method: &str, params: &str| format!(r#"{{"method": "{method}", "params": {params}}}"#);
        assert_eq!(
            parsed(&line("get-text", r#"{"start_row": 5, "rows": 2}"#)).method,
            Method::GetText {
                start_row: 5,
                rows: 2
            }
        );
        assert!(
            error(&line("get-text", r#"{"rows": 2}"#))
                .1
                .contains("start_row")
        );
        assert!(
            error(&line("get-text", r#"{"start_row": -1, "rows": 2}"#))
                .1
                .contains("start_row")
        );
        assert!(
            error(&line("get-text", r#"{"start_row": 0}"#))
                .1
                .contains("rows")
        );
        assert_eq!(
            parsed(&line("show-buffer", r#"{"buffer": 3}"#)).method,
            Method::ShowBuffer(3)
        );
        assert!(error(&line("show-buffer", "{}")).1.contains("\"buffer\""));
    }

    #[test]
    fn responses_are_single_json_lines() {
        let ok = success(&json!(1), json!({"a": "x\ny"}));
        assert!(!ok.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&ok).unwrap(),
            json!({"id": 1, "result": {"a": "x\ny"}})
        );
        let err = failure(&json!("q"), "bad\nthing");
        assert!(!err.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&err).unwrap(),
            json!({"id": "q", "error": {"message": "bad\nthing"}})
        );
    }

    #[test]
    fn requests_round_trip() {
        let line = request(9, "send-keys", Some(json!({"keys": "RET"})));
        assert_eq!(parsed(&line).id, json!(9));
        assert_eq!(
            parse(&request(1, "version", None)).unwrap().method,
            Method::Version
        );
    }
}
