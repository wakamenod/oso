//! The agent channel's methods (ADR 0085), over the JSON Lines framing in `wire`. There are
//! only these, and none of the dev socket's.
//!
//! Must not depend on: sockets, threads or editor state; it only parses requests.
//!
//! - `version`: the channel's protocol, which a client asks first.
//! - `state`: what the editor may show an agent; `{"buffer_offset": 100}` lists the buffers
//!   from the 101st.
//! - `commands`: the commands an agent may run, with their reach.
//! - `run`: `{"name": "forward-word", "arg": 3, "reach": "look"}` runs a `Look` or `Change`
//!   command; `arg` is optional, a count or a number, or text for a command that reads text,
//!   and `reach` must be the command's.
//! - `cancel`: closes a question the channel caused.
//! - `open`: `{"path": "/abs/notes.md", "line": 40}` visits a file that exists, shown once it
//!   is judged ordinary; `line` is optional.
//! - `text`: `{"buffer": 3, "start_row": 0, "rows": 200}` gives whole rows of a buffer, or
//!   `{"region": true}` the selected window's region.

use std::path::PathBuf;

use oso_core::buffer::Buffer;
use oso_core::command::Reach;
use serde_json::{Map, Value};

use super::wire::{self, BadRequest};

/// The channel's protocol version. Bump it on incompatible changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest command name `run` takes, in bytes.
const MAX_COMMAND_NAME: usize = 256;

/// The longest text `run` passes on, in bytes (ADR 0085).
pub const MAX_TEXT_ARG: usize = 64 * 1024;

/// The longest path `open` takes, in bytes.
pub const MAX_PATH: usize = 4096;

/// The rows `text` gives when not asked for a number, and the most it gives.
pub const DEFAULT_ROWS: usize = 200;
pub const MAX_ROWS: usize = 2000;

/// The methods, and the ones the dev socket has, which the channel must never answer. A
/// unit test checks that no name is in both.
#[cfg(test)]
pub const METHODS: [&str; 7] = [
    "version", "state", "commands", "run", "cancel", "open", "text",
];

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Echoed in the response.
    pub id: Value,
    pub method: Method,
}

/// What a request asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// The protocol version, which needs no UI.
    Version,
    /// Work for the UI thread.
    Ui(Op),
}

/// What the UI thread answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    State {
        buffer_offset: usize,
    },
    Commands,
    Run {
        name: String,
        arg: Option<Arg>,
        reach: Reach,
    },
    Cancel,
    Open {
        path: PathBuf,
        line: Option<usize>,
    },
    Text(TextOf),
}

/// What `text` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOf {
    /// Whole rows of a buffer, by id.
    Rows {
        buffer: u64,
        start_row: usize,
        rows: usize,
    },
    /// The selected window's region.
    Region,
}

/// An argument given with `run`. Its `Debug` gives a text's length only.
#[derive(Clone, PartialEq, Eq)]
pub enum Arg {
    Count(isize),
    Text(String),
}

impl std::fmt::Debug for Arg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count(n) => write!(f, "Count({n})"),
            Self::Text(text) => write!(f, "Text({} bytes)", text.len()),
        }
    }
}

/// Parses one request line.
pub fn parse(line: &str) -> Result<Request, BadRequest> {
    let parts = wire::envelope(line)?;
    let id = parts.id.clone();
    let bad = |message: String| BadRequest::new(&id, message);
    let none = |params: &Map<String, Value>| match params.keys().next() {
        Some(name) => Err(bad(format!(
            "{} takes no parameter {:?}",
            parts.method,
            short(name)
        ))),
        None => Ok(()),
    };
    let method = match parts.method.as_str() {
        "version" => {
            none(&parts.params)?;
            Method::Version
        }
        "state" => Method::Ui(parse_state(&parts.params).map_err(bad)?),
        "commands" => none(&parts.params).map(|()| Method::Ui(Op::Commands))?,
        "cancel" => none(&parts.params).map(|()| Method::Ui(Op::Cancel))?,
        "run" => Method::Ui(parse_run(&parts.params).map_err(bad)?),
        "open" => Method::Ui(parse_open(&parts.params).map_err(bad)?),
        "text" => Method::Ui(Op::Text(parse_text(&parts.params).map_err(bad)?)),
        _ => return Err(parts.unknown()),
    };
    Ok(Request { id, method })
}

/// The parameters of `state`: where its list of buffers starts.
fn parse_state(params: &Map<String, Value>) -> Result<Op, String> {
    if let Some(other) = params.keys().find(|name| name.as_str() != "buffer_offset") {
        return Err(format!("state takes no parameter {:?}", short(other)));
    }
    let buffer_offset = match params.get("buffer_offset") {
        None | Some(Value::Null) => 0,
        Some(offset) => offset
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("state's \"buffer_offset\" must be a whole number")?,
    };
    Ok(Op::State { buffer_offset })
}

/// The parameters of `run`.
fn parse_run(params: &Map<String, Value>) -> Result<Op, String> {
    if let Some(other) = params
        .keys()
        .find(|name| !["name", "arg", "reach"].contains(&name.as_str()))
    {
        return Err(format!("run takes no parameter {:?}", short(other)));
    }
    let Some(Value::String(name)) = params.get("name") else {
        return Err("run needs a string parameter \"name\"".into());
    };
    if name.len() > MAX_COMMAND_NAME {
        return Err(format!(
            "a command name is at most {MAX_COMMAND_NAME} bytes"
        ));
    }
    let arg = match params.get("arg") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.len() > MAX_TEXT_ARG => {
            return Err(format!("run's text is at most {MAX_TEXT_ARG} bytes"));
        }
        Some(Value::String(text)) => Some(Arg::Text(text.clone())),
        Some(arg) => Some(Arg::Count(
            arg.as_i64()
                .and_then(|n| isize::try_from(n).ok())
                .ok_or("run's \"arg\" must be a whole number or text")?,
        )),
    };
    let reach = match params.get("reach").and_then(Value::as_str) {
        Some("look") => Reach::Look,
        Some("change") => Reach::Change,
        _ => return Err("run needs \"reach\": \"look\" or \"change\"".into()),
    };
    Ok(Op::Run {
        name: name.clone(),
        arg,
        reach,
    })
}

/// The parameters of `open`. The path must be absolute, so that nothing depends on where
/// the editor runs, and its name must not say it may hold secrets.
fn parse_open(params: &Map<String, Value>) -> Result<Op, String> {
    only(params, "open", &["path", "line"])?;
    let Some(Value::String(path)) = params.get("path") else {
        return Err("open needs a string parameter \"path\"".into());
    };
    if path.len() > MAX_PATH {
        return Err(format!("a path is at most {MAX_PATH} bytes"));
    }
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("open needs an absolute path".into());
    }
    if Buffer::name_may_hold_secrets(&path) {
        return Err("the path's name says it may hold secrets; ask the person".into());
    }
    let line = match params.get("line") {
        None | Some(Value::Null) => None,
        Some(line) => Some(
            line.as_u64()
                .filter(|&line| line >= 1)
                .and_then(|line| usize::try_from(line).ok())
                .ok_or("open's \"line\" must be a whole number of at least 1")?,
        ),
    };
    Ok(Op::Open { path, line })
}

/// The parameters of `text`.
fn parse_text(params: &Map<String, Value>) -> Result<TextOf, String> {
    only(params, "text", &["buffer", "start_row", "rows", "region"])?;
    if params.get("region") == Some(&Value::Bool(true)) {
        if params.len() > 1 {
            return Err("text with \"region\" takes nothing else".into());
        }
        return Ok(TextOf::Region);
    }
    let size = |name: &str, default: Option<usize>| match params.get(name) {
        None if default.is_some() => Ok(default.unwrap_or_default()),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(format!("text's {name:?} must be a whole number")),
        None => Err(format!(
            "text needs a parameter {name:?}, or \"region\": true"
        )),
    };
    let rows = size("rows", Some(DEFAULT_ROWS))?;
    if rows > MAX_ROWS {
        return Err(format!("text gives at most {MAX_ROWS} rows"));
    }
    Ok(TextOf::Rows {
        buffer: size("buffer", None)? as u64,
        start_row: size("start_row", Some(0))?,
        rows,
    })
}

/// Refuses a parameter of `method` other than `names`.
fn only(params: &Map<String, Value>, method: &str, names: &[&str]) -> Result<(), String> {
    match params.keys().find(|name| !names.contains(&name.as_str())) {
        Some(other) => Err(format!("{method} takes no parameter {:?}", short(other))),
        None => Ok(()),
    }
}

/// A parameter name a client sent, cut short to echo.
fn short(name: &str) -> String {
    name.chars().take(32).collect()
}

#[cfg(test)]
mod tests {
    use super::{Arg, DEFAULT_ROWS, MAX_ROWS, MAX_TEXT_ARG, METHODS, Method, Op, TextOf, parse};
    use oso_core::command::Reach;
    use serde_json::Value;
    use serde_json::json;

    fn op(line: &str) -> Op {
        match parse(line).unwrap().method {
            Method::Ui(op) => op,
            Method::Version => panic!("version"),
        }
    }

    fn error(line: &str) -> String {
        parse(line).unwrap_err().message
    }

    #[test]
    fn parses_each_method() {
        assert_eq!(
            parse(r#"{"id": 1, "method": "version"}"#).unwrap().method,
            Method::Version
        );
        assert_eq!(
            parse(r#"{"id": 1, "method": "version"}"#).unwrap().id,
            json!(1)
        );
        assert_eq!(op(r#"{"method": "state"}"#), Op::State { buffer_offset: 0 });
        assert_eq!(
            op(r#"{"method": "state", "params": {"buffer_offset": 100}}"#),
            Op::State { buffer_offset: 100 }
        );
        assert_eq!(op(r#"{"method": "commands"}"#), Op::Commands);
        assert_eq!(op(r#"{"method": "cancel"}"#), Op::Cancel);
        assert_eq!(
            op(
                r#"{"method": "run", "params": {"name": "forward-word", "arg": -3, "reach": "look"}}"#
            ),
            Op::Run {
                name: "forward-word".into(),
                arg: Some(Arg::Count(-3)),
                reach: Reach::Look
            }
        );
        assert_eq!(
            op(r#"{"method": "run", "params": {"name": "undo", "reach": "change"}}"#),
            Op::Run {
                name: "undo".into(),
                arg: None,
                reach: Reach::Change
            }
        );
    }

    #[test]
    fn refuses_what_it_does_not_know() {
        assert!(error(r#"{"method": "run", "params": {"name": "undo"}}"#).contains("reach"));
        assert!(
            error(r#"{"method": "run", "params": {"name": "undo", "reach": "person"}}"#)
                .contains("reach")
        );
        assert!(error(r#"{"method": "run", "params": {"reach": "look"}}"#).contains("name"));
        assert!(
            error(r#"{"method": "run", "params": {"name": "a", "reach": "look", "arg": 1.5}}"#)
                .contains("whole number")
        );
        assert!(
            error(r#"{"method": "run", "params": {"name": "a", "reach": "look", "keys": "y"}}"#)
                .contains("no parameter")
        );
        let long =
            json!({ "method": "run", "params": { "name": "a".repeat(257), "reach": "look" } });
        assert!(error(&long.to_string()).contains("at most"));
        assert!(error(r#"{"method": "state", "params": {"x": 1}}"#).contains("no parameter"));
    }

    #[test]
    fn run_takes_text_up_to_its_limit_and_hides_it_in_debug() {
        let line = json!({ "method": "run", "params": { "name": "insert", "arg": "secret-ish", "reach": "change" } });
        let parsed = op(&line.to_string());
        assert_eq!(
            parsed,
            Op::Run {
                name: "insert".into(),
                arg: Some(Arg::Text("secret-ish".into())),
                reach: Reach::Change
            }
        );
        assert!(!format!("{parsed:?}").contains("secret-ish"));
        let long = json!({ "method": "run", "params": { "name": "insert", "arg": "a".repeat(MAX_TEXT_ARG + 1), "reach": "change" } });
        assert!(error(&long.to_string()).contains("at most"));
    }

    // Its paths are Unix ones, and the channel has no transport elsewhere yet (ADR 0032).
    #[cfg(unix)]
    #[test]
    fn open_takes_an_absolute_path_whose_name_is_ordinary() {
        let open = |params: Value| json!({ "method": "open", "params": params }).to_string();
        assert_eq!(
            op(&open(json!({ "path": "/tmp/notes.md", "line": 40 }))),
            Op::Open {
                path: "/tmp/notes.md".into(),
                line: Some(40)
            }
        );
        assert!(error(&open(json!({ "path": "notes.md" }))).contains("absolute"));
        assert!(error(&open(json!({ "path": "/home/a/.env" }))).contains("may hold secrets"));
        assert!(
            error(&open(json!({ "path": "/home/a/.ssh/config" }))).contains("may hold secrets")
        );
        assert!(error(&open(json!({ "path": "/a", "line": 0 }))).contains("at least 1"));
        let long = format!("/{}", "a".repeat(super::MAX_PATH));
        assert!(error(&open(json!({ "path": long }))).contains("at most"));
    }

    #[test]
    fn text_gives_rows_of_a_buffer_or_the_region() {
        let text = |params: Value| json!({ "method": "text", "params": params }).to_string();
        assert_eq!(
            op(&text(json!({ "buffer": 3 }))),
            Op::Text(TextOf::Rows {
                buffer: 3,
                start_row: 0,
                rows: DEFAULT_ROWS
            })
        );
        assert_eq!(
            op(&text(json!({ "region": true }))),
            Op::Text(TextOf::Region)
        );
        assert!(error(&text(json!({}))).contains("\"buffer\""));
        assert!(error(&text(json!({ "buffer": 1, "rows": MAX_ROWS + 1 }))).contains("at most"));
        assert!(error(&text(json!({ "region": true, "buffer": 1 }))).contains("nothing else"));
    }

    /// The channel answers none of the dev socket's methods, whatever the build.
    #[test]
    fn no_dev_method_reaches_the_channel() {
        let dev = [
            "send-keys",
            "screenshot",
            "get-state",
            "get-text",
            "run-command",
            "show-buffer",
            "check-handover",
            "export-state",
            "import-state",
            "exit",
            "take-timings",
            "open-file",
            "dev-panic",
            "dev-end-unasked",
            "ime",
            "clipboard",
        ];
        for method in dev {
            assert!(!METHODS.contains(&method), "{method}");
            let message = error(&json!({ "method": method }).to_string());
            assert!(message.starts_with("unknown method"), "{method}: {message}");
        }
    }
}
