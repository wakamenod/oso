//! The control protocol's lines, tier 2 of ADR 0165 ("The route"): the requests oso writes on a
//! native session's standard input, and what the CLI's `control_response` says of one.
//!
//! Must not depend on: `rows`, threads or the UI; plain functions over a parsed line.
//!
//! - Only `initialize`, `interrupt` (step 2c) and `get_usage` (part 3a) are written, each
//!   with an id of oso's, `oso-N`: an answer whose id is not one of these is no answer of
//!   oso's.
//! - Of an answer only its id, whether it succeeded, its error, cut to [`super::MAX_NAME`],
//!   the slash commands it lists (`commands`, step 2d), and what the status line reads (the
//!   models, the usage, part 3a, `status_line`) are read: what `initialize` answers otherwise (the
//!   account, the agents) is not kept.
//! - A `control_request` of the CLI's is never answered, as oso asks it for none: its hooks,
//!   its permission questions and its SDK servers are not oso's (permissions go through the
//!   bridge, ADR 0165, Q2).

use serde_json::{Map, Value};

use super::slash::{SlashCommand, commands};
use super::status_line::{Model, Usage, models, usage};
use super::string;

/// A request oso writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Written first, as the process starts.
    Initialize,
    /// Ends the turn under way, keeping the process.
    Interrupt,
    /// Asks what the session cost and how much of the rate limits is used (part 3a).
    GetUsage,
}

/// What a `control_response` says of a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The request's id, when it is one of oso's.
    pub id: Option<u64>,
    /// Why it failed, cut to [`super::MAX_NAME`]; `None` when it succeeded.
    pub error: Option<String>,
    /// The slash commands it lists, as `initialize`'s does (step 2d).
    pub commands: Option<Vec<SlashCommand>>,
    /// The models it lists, as `initialize`'s does, and the usage `get_usage`'s gives
    /// (part 3a).
    pub models: Option<Vec<Model>>,
    pub usage: Option<Usage>,
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
        Request::GetUsage => "get_usage",
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
    let body = response.get("response").and_then(Value::as_object);
    Some(Answer {
        id,
        error,
        commands: body.and_then(commands),
        models: body.and_then(models),
        usage: body.and_then(usage),
    })
}
