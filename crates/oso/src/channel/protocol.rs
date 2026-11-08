//! The agent channel's methods (ADR 0085), over the JSON Lines framing in `wire`. There are
//! only these, and none of the dev socket's.
//!
//! Must not depend on: sockets, threads or editor state; it only parses requests.
//!
//! - `version`: the channel's protocol, which a client asks first.
//! - `state`: what the editor may show an agent.
//! - `commands`: the commands an agent may run, with their reach.
//! - `run`: `{"name": "forward-word", "arg": 3, "reach": "look"}` runs a `Look` or `Change`
//!   command; `arg` is optional, and `reach` must be the command's.
//! - `cancel`: closes a question the channel caused.

use oso_core::command::Reach;
use serde_json::{Map, Value};

use super::wire::{self, BadRequest};

/// The channel's protocol version. Bump it on incompatible changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest command name `run` takes, in bytes.
const MAX_COMMAND_NAME: usize = 256;

/// The methods, and the ones the dev socket has, which the channel must never answer. A
/// unit test checks that no name is in both.
#[cfg(test)]
pub const METHODS: [&str; 5] = ["version", "state", "commands", "run", "cancel"];

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
    State,
    Commands,
    Run {
        name: String,
        arg: Option<isize>,
        reach: Reach,
    },
    Cancel,
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
        "state" => none(&parts.params).map(|()| Method::Ui(Op::State))?,
        "commands" => none(&parts.params).map(|()| Method::Ui(Op::Commands))?,
        "cancel" => none(&parts.params).map(|()| Method::Ui(Op::Cancel))?,
        "run" => Method::Ui(parse_run(&parts.params).map_err(bad)?),
        _ => return Err(parts.unknown()),
    };
    Ok(Request { id, method })
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
        Some(arg) => Some(
            arg.as_i64()
                .and_then(|n| isize::try_from(n).ok())
                .ok_or("run's \"arg\" must be a whole number")?,
        ),
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

/// A parameter name a client sent, cut short to echo.
fn short(name: &str) -> String {
    name.chars().take(32).collect()
}

#[cfg(test)]
mod tests {
    use super::{METHODS, Method, Op, parse};
    use oso_core::command::Reach;
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
        assert_eq!(op(r#"{"method": "state"}"#), Op::State);
        assert_eq!(op(r#"{"method": "commands"}"#), Op::Commands);
        assert_eq!(op(r#"{"method": "cancel"}"#), Op::Cancel);
        assert_eq!(
            op(
                r#"{"method": "run", "params": {"name": "forward-word", "arg": -3, "reach": "look"}}"#
            ),
            Op::Run {
                name: "forward-word".into(),
                arg: Some(-3),
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
