//! JSON Lines, as the dev control socket and the agent channel speak it (ADR 0023,
//! ADR 0085): one JSON object per line of UTF-8, ended by `\n`.
//!
//! Must not depend on: `app`, editor state or what a method means; this frames requests and
//! responses, and reads lines within their limits.
//!
//! - Request: `{"id": 1, "method": "state", "params": {...}}`. `id` is optional and may be a
//!   number, a string or null. `params` is optional.
//! - Response: `{"id": 1, "result": ...}` or `{"id": 1, "error": {"message": "..."}}`,
//!   echoing the request's `id`.

use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader};
use std::time::{Duration, Instant};

use oso_core::display_map::cell::is_invisible;
use oso_platform::control::ControlStream;
use serde_json::{Map, Value, json};

/// The longest request or response line, without its newline.
pub const MAX_LINE: usize = 1 << 20;

/// The longest string `id`, in bytes. Ids are echoed, so this keeps responses small.
pub const MAX_ID: usize = 256;

/// The most characters of a method name echoed in an error.
const MAX_ECHO: usize = 64;

/// A request's parts, before its method is known.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    /// Echoed in the response.
    pub id: Value,
    pub method: String,
    pub params: Map<String, Value>,
    /// The request's other members, which a method may refuse.
    pub rest: Map<String, Value>,
}

/// A request that cannot be handled: the `id` to answer with, if one was found, and the
/// error message.
#[derive(Debug, Clone, PartialEq)]
pub struct BadRequest {
    pub id: Value,
    pub message: String,
}

impl BadRequest {
    /// An error answering `id`.
    pub fn new(id: &Value, message: impl Into<String>) -> Self {
        Self {
            id: id.clone(),
            message: message.into(),
        }
    }
}

/// Parses one request line into its parts.
pub fn envelope(line: &str) -> Result<Envelope, BadRequest> {
    let value: Value = serde_json::from_str(line)
        .map_err(|e| BadRequest::new(&Value::Null, format!("invalid JSON: {e}")))?;
    let Value::Object(mut request) = value else {
        return Err(BadRequest::new(
            &Value::Null,
            "the request is not a JSON object",
        ));
    };
    let id = match request.remove("id") {
        None => Value::Null,
        Some(Value::String(id)) if id.len() > MAX_ID => {
            return Err(BadRequest::new(
                &Value::Null,
                format!("id is longer than {MAX_ID} bytes"),
            ));
        }
        Some(id @ (Value::Null | Value::Number(_) | Value::String(_))) => id,
        Some(_) => {
            return Err(BadRequest::new(
                &Value::Null,
                "id must be a number, a string or null",
            ));
        }
    };
    let Some(Value::String(method)) = request.remove("method") else {
        return Err(BadRequest::new(&id, "method must be a string"));
    };
    let params = match request.remove("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(params)) => params,
        Some(_) => return Err(BadRequest::new(&id, "params must be an object")),
    };
    Ok(Envelope {
        id,
        method,
        params,
        rest: request,
    })
}

impl Envelope {
    /// The error for a method no one knows, quoting a short prefix of its name.
    pub fn unknown(&self) -> BadRequest {
        let mut shown: String = self.method.chars().take(MAX_ECHO).collect();
        if shown.len() < self.method.len() {
            shown.push('…');
        }
        BadRequest::new(&self.id, format!("unknown method {shown:?}"))
    }
}

/// A success response line, without the newline.
pub fn success(id: &Value, result: Value) -> String {
    escape_unsafe(json!({ "id": id, "result": result }).to_string())
}

/// An error response line, without the newline.
pub fn failure(id: &Value, message: &str) -> String {
    escape_unsafe(json!({ "id": id, "error": { "message": message } }).to_string())
}

/// `line`, or an error in its place if a client could not read it: clients stop at
/// [`MAX_LINE`] (ADR 0032).
pub fn bounded(id: &Value, line: String) -> String {
    if line.len() > MAX_LINE {
        let message = format!(
            "the response takes {} bytes, more than the {MAX_LINE} a line may hold; ask for less",
            line.len()
        );
        return failure(id, &message);
    }
    line
}

/// `line` with what JSON leaves raw, but a terminal or a reader should not see raw, as
/// `\uXXXX`: DEL, the C1 controls (U+009B starts an escape sequence on some terminals), and
/// what the editor itself draws as a boxed code point, such as bidi controls and invisible
/// tag characters. File text reaches clients' output, so a file must not be able to act on
/// the terminal, or to say something its reader cannot see (security.md, T3).
///
/// These never occur outside a JSON string, and an escape there means the same character,
/// so the line parses to the same value.
pub fn escape_unsafe(line: String) -> String {
    let is_unsafe = |c: char| matches!(c, '\u{7f}'..='\u{9f}') || is_invisible(c);
    if !line.contains(is_unsafe) {
        return line;
    }
    let mut escaped = String::with_capacity(line.len());
    for c in line.chars() {
        if is_unsafe(c) {
            let mut units = [0; 2];
            for unit in c.encode_utf16(&mut units) {
                write!(escaped, "\\u{unit:04x}").expect("writing to a String cannot fail");
            }
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// The result in a response line, or its error message. Anything else is an error too, so
/// that a response this build does not understand never reads as a confirmation.
#[cfg(any(test, feature = "dev"))]
pub fn response_result(line: &str) -> Result<Value, String> {
    let mut response: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if let Some(message) = response["error"]["message"].as_str() {
        return Err(message.to_owned());
    }
    match response.get_mut("result") {
        Some(result) => Ok(result.take()),
        None => Err("a response with no result".to_owned()),
    }
}

/// Builds a request line, without the newline. For clients and tests.
#[cfg(any(test, feature = "dev"))]
pub fn request(id: u64, method: &str, params: Option<Value>) -> String {
    let mut request = json!({ "id": id, "method": method });
    if let Some(params) = params {
        request["params"] = params;
    }
    request.to_string()
}

/// Reads up to a newline into `line`, at most [`MAX_LINE`] + 1 bytes, and returns how many.
/// Times out if the whole line takes longer than `within`, so a client cannot hold its slot
/// by sending a byte now and then.
pub fn read_line_within(
    reader: &mut BufReader<ControlStream>,
    line: &mut Vec<u8>,
    within: Duration,
) -> io::Result<usize> {
    let started = Instant::now();
    loop {
        let remaining = within
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(io::ErrorKind::TimedOut)?;
        reader.get_ref().set_read_timeout(Some(remaining))?;
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(line.len());
        }
        let room = MAX_LINE + 1 - line.len();
        let window = &available[..available.len().min(room)];
        let newline = window.iter().position(|&byte| byte == b'\n');
        let used = newline.map_or(window.len(), |at| at + 1);
        line.extend_from_slice(&window[..used]);
        reader.consume(used);
        if newline.is_some() || line.len() > MAX_LINE {
            return Ok(line.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BadRequest, MAX_ID, envelope, failure, request, response_result, success};
    use serde_json::{Value, json};

    #[test]
    fn parses_the_parts_of_a_request() {
        let parts = envelope(r#"{"id": 3, "method": "state", "params": {"a": 1}}"#).unwrap();
        assert_eq!(parts.id, json!(3));
        assert_eq!(parts.method, "state");
        assert_eq!(parts.params["a"], json!(1));
        let bare = envelope(r#"{"method": "version"}"#).unwrap();
        assert_eq!((bare.id, bare.params.len()), (Value::Null, 0));
    }

    #[test]
    fn refuses_what_is_not_a_request() {
        let message = |line: &str| envelope(line).unwrap_err().message;
        assert!(message("nope").starts_with("invalid JSON"));
        assert_eq!(message("[1]"), "the request is not a JSON object");
        assert_eq!(
            message(r#"{"id": [1], "method": "x"}"#),
            "id must be a number, a string or null"
        );
        assert_eq!(message(r#"{"id": 1}"#), "method must be a string");
        assert_eq!(
            message(r#"{"method": "x", "params": 1}"#),
            "params must be an object"
        );
        let long = format!(r#"{{"id": "{}", "method": "x"}}"#, "a".repeat(MAX_ID + 1));
        assert_eq!(
            envelope(&long).unwrap_err(),
            BadRequest::new(&Value::Null, format!("id is longer than {MAX_ID} bytes"))
        );
    }

    #[test]
    fn names_an_unknown_method_in_short() {
        let parts = envelope(&request(1, &"m".repeat(100), None)).unwrap();
        let message = parts.unknown().message;
        assert!(message.ends_with("…\""), "{message}");
        assert!(message.len() < 90, "{message}");
    }

    #[test]
    fn responses_parse_back_and_hide_nothing() {
        let line = success(&json!(1), json!({ "text": "a\u{202e}b\u{85}" }));
        assert!(
            line.contains("\\u202e") && line.contains("\\u0085"),
            "{line}"
        );
        assert_eq!(response_result(&line).unwrap()["text"], "a\u{202e}b\u{85}");
        assert_eq!(
            response_result(&failure(&json!(1), "no")),
            Err("no".to_owned())
        );
        assert!(response_result(r#"{"id": 1}"#).is_err());
    }
}
