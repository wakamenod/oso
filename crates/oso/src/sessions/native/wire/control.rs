//! The control protocol's lines, tier 2 of ADR 0165 ("The route"): the requests oso writes on a
//! native session's standard input, and what the CLI's `control_response` says of one.
//!
//! Must not depend on: `rows`, threads or the UI; plain functions over a parsed line.
//!
//! - Only `initialize` and `interrupt` are written (step 2c), each with an id of oso's,
//!   `oso-N`: an answer whose id is not one of these is no answer of oso's.
//! - Of an answer only its id, whether it succeeded, its error, cut to [`super::MAX_NAME`],
//!   and the slash commands it lists (`commands`, step 2d) are read: what `initialize` answers
//!   otherwise (the models, the account) is not kept.
//! - A `control_request` of the CLI's is never answered, as oso asks it for none: its hooks,
//!   its permission questions and its SDK servers are not oso's (permissions go through the
//!   bridge, ADR 0165, Q2).

use serde_json::{Map, Value};

use super::slash::{SlashCommand, commands};
use super::string;

/// A request oso writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Written first, as the process starts.
    Initialize,
    /// Ends the turn under way, keeping the process.
    Interrupt,
}

/// What a `control_response` says of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The request's id, when it is one of oso's.
    pub id: Option<u64>,
    /// Why it failed, cut to [`super::MAX_NAME`]; `None` when it succeeded.
    pub error: Option<String>,
    /// The slash commands it lists, as `initialize`'s does (step 2d).
    pub commands: Option<Vec<SlashCommand>>,
}

/// The id of `initialize`, the first request; the others have the ids after it.
pub const INITIALIZE: u64 = 0;

/// The prefix of oso's request ids.
const PREFIX: &str = "oso-";

/// The line that writes `request` with id `id`, ending in `\n`.
pub fn request(id: u64, request: Request) -> String {
    let subtype = match request {
        Request::Initialize => "initialize",
        Request::Interrupt => "interrupt",
    };
    let line = serde_json::json!({
        "type": "control_request",
        "request_id": format!("{PREFIX}{id}"),
        "request": { "subtype": subtype },
    });
    let mut line = line.to_string();
    line.push('\n');
    line
}

/// What a `control_response` line, read into `object`, says; `None` when it holds no
/// response.
pub(super) fn answer(object: &Map<String, Value>) -> Option<Answer> {
    let response = object.get("response")?.as_object()?;
    let id = string(response, "request_id");
    let id = id.strip_prefix(PREFIX).and_then(|n| n.parse().ok());
    let error = match string(response, "subtype").as_str() {
        "success" => None,
        _ => Some(string(response, "error")).map(|error| match error.is_empty() {
            true => "no reason given".to_owned(),
            false => error,
        }),
    };
    let commands = (response.get("response").and_then(Value::as_object)).and_then(commands);
    Some(Answer {
        id,
        error,
        commands,
    })
}
