//! What the stream says of Remote Control (ADR 0181): the answer to oso's `remote_control`
//! request, `system/bridge_state`, the `command_lifecycle` lines of a message that came another
//! way than oso's standard input, and the `origin` of a message.
//!
//! Must not depend on: `rows`, threads or the UI; plain functions over a parsed line.
//!
//! - A word is read whole and as the CLI writes it, or not at all: a state in another case,
//!   cut, or of another type is none of these.
//! - Of the answer only the session's address and its id are read, each only of exactly the
//!   shape claude.ai's have ([`SessionUrl`], [`RemoteId`]); `connect_url` and the environment
//!   are never read. Neither is printed by `Debug`, so neither reaches a log.
//! - What these lines may come to is `native::remote`'s: the stream is not authenticated.

use std::fmt;

use serde_json::{Map, Value};

use super::{string, whole};
use crate::sessions::identity::SessionId;

/// The most of an id's letters and digits after its prefix.
const MAX_LETTERS: usize = 64;
/// How the address of a session on claude.ai starts.
const URL: &str = "https://claude.ai/code/session_";
/// How the session's id starts.
const ID: &str = "cse_";

/// Where the connection is, as `system/bridge_state` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// The session on claude.ai is made or found; its connection is not up yet.
    Ready,
    Connected,
    /// The connection is down, and the CLI tries again.
    Reconnecting,
    Failed,
    /// The organization's policy refuses it.
    PolicyDisabled,
}

/// A `system/bridge_state` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub state: ConnectionState,
    /// Why, as the CLI says it, cut to [`super::MAX_NAME`]: shown only escaped.
    pub detail: String,
}

/// Where a message that came with an id of its own is in the CLI's queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Queued, or its turn started: its replay comes.
    Comes,
    /// Its turn ended, or it never had one.
    Over,
}

/// A `command_lifecycle` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifecycle {
    /// The message's id, which its replay has.
    pub uuid: String,
    pub stage: Stage,
}

/// Where a message came from, as its line's `origin` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// It does not say, as of a message oso sent (measured, CLI 2.1.330).
    #[default]
    Unsaid,
    /// A person, as of a message sent from claude.ai.
    Human,
    /// Anything else: another session, a task, or a kind not known here.
    Other,
}

/// The session's address on claude.ai: `https://claude.ai/code/session_` and letters and
/// digits, nothing more.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionUrl(String);

/// The id `remote_control` answers for the session: `cse_` and letters and digits.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteId(String);

/// What the answer that turned Remote Control on gives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Enabled {
    pub url: Option<SessionUrl>,
    pub id: Option<RemoteId>,
}

/// How a message a process replays came to it, as `native::remote` judged it, never by the line
/// alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Came {
    /// As the line reads: Remote Control is off.
    #[default]
    AsSaid,
    /// From claude.ai: never the person's message at oso.
    Remote,
    /// From another session, or the CLI's own: never the person's message at oso.
    Elsewhere,
}

/// A request to turn it on that `native::remote` began: only with one is its line written.
#[derive(Debug, PartialEq, Eq)]
pub struct Asked(u64);

impl Asked {
    /// Minted by [`crate::sessions::native::remote::Remote::begin`] alone.
    pub(in crate::sessions::native) fn new(id: u64) -> Self {
        Self(id)
    }

    /// The request's id.
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// What the request that turns Remote Control on asks beside that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Enable<'a> {
    /// What the session is called on claude.ai.
    pub name: Option<&'a str>,
    /// The session there to attach to again, as it was answered before.
    pub reattach: Option<&'a RemoteId>,
    /// The session there is kept as the process exits, to be attached to again.
    pub keep: bool,
}

impl SessionUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RemoteId {
    /// The id `text` is, of exactly its shape: one the parent's restart state carried.
    pub fn read(text: &str) -> Option<Self> {
        shaped(text, ID).then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionUrl(..)")
    }
}

impl fmt::Debug for RemoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteId(..)")
    }
}

/// The body of the request that turns Remote Control on as `enable` says, or off.
pub(super) fn request(enable: Option<&Enable<'_>>) -> Value {
    let mut body = serde_json::json!({ "subtype": "remote_control", "enabled": enable.is_some() });
    let Some(enable) = enable else {
        return body;
    };
    if let Some(name) = enable.name {
        body["name"] = Value::from(name);
    }
    if let Some(reattach) = enable.reattach {
        body["reattach_session_id"] = Value::from(reattach.as_str());
    }
    if enable.keep {
        body["keep_session_on_exit"] = Value::Bool(true);
    }
    body
}

/// What the answer `response` gives of a session turned on, if it gives any of it.
pub(super) fn enabled(response: &Map<String, Value>) -> Option<Enabled> {
    let shaped = |key, prefix: &str| {
        let text = response.get(key)?.as_str()?;
        shaped(text, prefix).then(|| text.to_owned())
    };
    let enabled = Enabled {
        url: shaped("session_url", URL).map(SessionUrl),
        id: shaped("bridge_session_id", ID).map(RemoteId),
    };
    (enabled != Enabled::default()).then_some(enabled)
}

/// Whether `text` is `prefix` and then one to [`MAX_LETTERS`] ASCII letters and digits.
fn shaped(text: &str, prefix: &str) -> bool {
    text.strip_prefix(prefix).is_some_and(|rest| {
        (1..=MAX_LETTERS).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// A `system/bridge_state` line, if its state is one known here.
pub(super) fn connection(object: &Map<String, Value>) -> Option<Connection> {
    let state = match object.get("state")?.as_str()? {
        "ready" => ConnectionState::Ready,
        "connected" => ConnectionState::Connected,
        "reconnecting" => ConnectionState::Reconnecting,
        "failed" => ConnectionState::Failed,
        "policy_disabled" => ConnectionState::PolicyDisabled,
        _ => return None,
    };
    Some(Connection {
        state,
        detail: string(object, "detail"),
    })
}

/// A `command_lifecycle` line, if its id can be one and its state is one known here.
pub(super) fn lifecycle(object: &Map<String, Value>) -> Option<Lifecycle> {
    let stage = match object.get("state")?.as_str()? {
        "queued" | "started" => Stage::Comes,
        "completed" | "cancelled" | "discarded" | "refused" => Stage::Over,
        _ => return None,
    };
    let uuid = whole(object, "command_uuid");
    // A session's id is a UUID as the CLI writes one, as a message's is.
    SessionId::parse(&uuid).map(|_| Lifecycle { uuid, stage })
}

/// The origin of a message's line. One whose `origin` is there but does not read is
/// [`Origin::Other`].
pub(super) fn origin(object: &Map<String, Value>) -> Origin {
    match object.get("origin") {
        None | Some(Value::Null) => Origin::Unsaid,
        Some(Value::Object(origin))
            if origin.get("kind").and_then(Value::as_str) == Some("human") =>
        {
            Origin::Human
        }
        Some(_) => Origin::Other,
    }
}

#[cfg(test)]
mod tests;
