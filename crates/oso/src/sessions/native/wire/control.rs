//! The control protocol's lines, tier 2 of ADR 0165 ("The route"): the requests oso writes on a
//! native session's standard input, and what the CLI's `control_response` says of one.
//!
//! Must not depend on: `rows`, threads or the UI; plain functions over a parsed line.
//!
//! - Only `initialize`, `interrupt` (step 2c), `get_usage` (part 3a),
//!   `set_permission_mode` (part 3b, with one of [`PermissionMode`]'s) and `remote_control`
//!   (ADR 0181, at the person's command alone) are written, each with an id of oso's, `oso-N`: an
//!   answer whose id is not one of these is no answer of oso's.
//! - Of an answer only its id, whether it succeeded, its error, cut to [`super::MAX_NAME`],
//!   the slash commands it lists (`commands`, step 2d), and what the status line reads (the
//!   models, the usage, part 3a, `status_line`), the permission mode `initialize` says
//!   the session is in (part 3b), whether it says Remote Control is available, and what
//!   `remote_control`'s gives (ADR 0181, `remote`) are read: what `initialize` answers otherwise
//!   (the account, the agents) is not kept.
//! - A `control_request` of the CLI's is never answered, as oso asks it for none: its hooks,
//!   its permission questions and its SDK servers are not oso's (permissions go through the
//!   bridge, ADR 0165, Q2).

use serde_json::{Map, Value};

use super::remote::{self, Asked, Enable, Enabled};
use super::slash::{SlashCommand, commands};
use super::status_line::{Model, Usage, models, usage};
use super::{string, whole};
use crate::channel::protocol::mode::PermissionMode;

/// A request oso writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Written first, as the process starts.
    Initialize,
    /// Ends the turn under way, keeping the process.
    Interrupt,
    /// Asks what the session cost and how much of the rate limits is used (part 3a).
    GetUsage,
    /// Puts the session in this permission mode (part 3b).
    SetPermissionMode(PermissionMode),
    /// Remote Control (ADR 0181): [`request`] writes only that it is turned off, before its
    /// process is ended; [`enable`] alone turns it on.
    RemoteControl,
}

/// What a `control_response` says of a request.
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// The permission mode the session is in, as `initialize`'s says it (part 3b): the CLI's
    /// word, whole, or [`super::TOO_LONG`] for one longer than [`super::MAX_NAME`].
    pub mode: Option<String>,
    /// Whether Remote Control is available, as `initialize`'s says (ADR 0181).
    pub remote_available: Option<bool>,
    /// What `remote_control`'s gives of the session on claude.ai (ADR 0181).
    pub remote: Option<Box<Enabled>>,
}

/// The id of `initialize`, the first request; the others have the ids after it.
pub const INITIALIZE: u64 = 0;

/// The prefix of oso's request ids.
const PREFIX: &str = "oso-";

/// The line that writes `request` with id `id`, ending in `\n`.
pub fn request(id: u64, request: Request) -> String {
    let plain = |subtype: &str| serde_json::json!({ "subtype": subtype });
    line(
        id,
        &match request {
            Request::Initialize => plain("initialize"),
            Request::Interrupt => plain("interrupt"),
            Request::GetUsage => plain("get_usage"),
            Request::SetPermissionMode(mode) => {
                serde_json::json!({ "subtype": "set_permission_mode", "mode": mode.name() })
            }
            Request::RemoteControl => remote::request(None),
        },
    )
}

/// The line that asks to connect the session to claude.ai as `enable` says, for the request
/// `asked`, ending in `\n` (ADR 0181).
pub fn enable(asked: &Asked, enable: &Enable<'_>) -> String {
    line(asked.id(), &remote::request(Some(enable)))
}

/// The line of a request whose body is `body`, with id `id`, ending in `\n`.
fn line(id: u64, body: &Value) -> String {
    let line = serde_json::json!({
        "type": "control_request",
        "request_id": format!("{PREFIX}{id}"),
        "request": body,
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
        mode: body
            .map(|body| whole(body, "current_permission_mode"))
            .filter(|mode| !mode.is_empty()),
        remote_available: body
            .and_then(|body| body.get("remote_control_available"))
            .and_then(Value::as_bool),
        remote: body.and_then(remote::enabled).map(Box::new),
    })
}
