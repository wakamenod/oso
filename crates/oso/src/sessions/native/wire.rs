//! One line of `claude -p --output-format stream-json --verbose --include-partial-messages
//! --replay-user-messages` read into what a native session shows (ADR 0165), and the line that
//! sends the person's message on its standard input.
//!
//! Must not depend on: threads, files or the UI; plain functions over a line's bytes.
//!
//! - Only a small, named set of fields is read; every other field is ignored, so a field the
//!   CLI adds changes nothing. A line that is not one JSON object, or nests deeper than
//!   `serde_json`'s limit, is [`Event::Unreadable`]; a type or subtype not known here is
//!   [`Event::Other`], which shows nothing.
//! - A line is at most [`MAX_LINE`] bytes; the reader that splits the stream drops longer ones
//!   as unreadable. Each text kept is cut to its own limit, so what an event holds is bounded
//!   whatever the line.
//! - The transcript format and the control protocol are not read here: only the stream.
//! - `messages` reads the messages' bodies and `tools` a tool call's input: with this file, the
//!   only places that touch JSON.

mod messages;
pub mod tools;

use serde_json::{Map, Value};

pub(crate) use super::capped;
use messages::{assistant, partial, rate_limit, result, user};
pub use tools::ToolInput;

/// The longest line read, in bytes (ADR 0165, "Code").
pub const MAX_LINE: usize = 16 << 20;
/// The most of a message's text, a thinking block's or a tool result's kept, in bytes.
pub const MAX_TEXT: usize = 1 << 20;
/// The most of a name, an id, a model or a reason kept, in bytes.
pub const MAX_NAME: usize = 256;
/// The most diff lines kept of one tool result.
pub const MAX_DIFF_LINES: usize = 20_000;
/// The most bytes of diff lines kept of one tool result, in all.
pub const MAX_DIFF_BYTES: usize = MAX_TEXT;

/// What one line says.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `system/init`, sent with each turn.
    Init(Init),
    /// `system/status`.
    Status(Status),
    /// `system/compact_boundary`: the conversation was compacted.
    Compacted {
        pre_tokens: Option<u64>,
        post_tokens: Option<u64>,
        manual: bool,
    },
    /// `system/permission_denied`: a tool call was refused before it ran.
    Denied {
        tool_use_id: String,
        message: String,
    },
    /// A `system/task_*` line about a subagent or a background task.
    Task(Task),
    /// A `stream_event`: part of a message as it streams.
    Partial(Partial),
    /// An `assistant` message: its blocks, whole.
    Assistant(Assistant),
    /// A `user` message: the person's, replayed, or tool results.
    User(User),
    /// `result`: a turn ended.
    Result(TurnResult),
    /// `rate_limit_event`.
    RateLimit(RateLimit),
    /// A line of a known shape that shows nothing, named by its type and subtype.
    Other(String),
    /// A line that is not one JSON object: its length.
    Unreadable { bytes: usize },
}

/// What `system/init` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Init {
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    pub permission_mode: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

/// What `system/status` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Requesting,
    Compacting,
    /// Compacting ended, well or not.
    Compacted {
        ok: bool,
    },
    /// No status, or one not known here.
    Other,
}

/// A `system/task_*` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// The tool call that started the task, if it says.
    pub tool_use_id: Option<String>,
    /// What it is doing now, or its description.
    pub description: Option<String>,
    /// `completed`, `failed`, `stopped` and the like, once it says.
    pub status: Option<String>,
}

/// A `stream_event`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partial {
    /// The id of the message it is part of.
    pub message_id: String,
    /// The tool call whose subagent it comes from, if any.
    pub parent: Option<String>,
    pub part: PartKind,
}

/// What a `stream_event` carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartKind {
    /// A block starts.
    Start { index: u64, block: BlockKind },
    /// More of a block's text, thinking or input.
    Delta { index: u64, text: String },
    /// A block ended.
    Stop { index: u64 },
    /// Any other stream event: a message's start, delta or stop.
    Other,
}

/// What kind of block a message holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
    Other,
}

/// An `assistant` message.
#[derive(Clone, Debug, PartialEq)]
pub struct Assistant {
    pub message_id: String,
    pub parent: Option<String>,
    pub model: String,
    pub blocks: Vec<Block>,
}

/// A block of an assistant's message.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(String),
    /// Thinking: its text, which the CLI may leave empty, and how long it took.
    Thinking {
        text: String,
        millis: Option<u64>,
    },
    /// A tool call: its id, its name and what its input works on.
    ToolUse {
        id: String,
        name: String,
        input: ToolInput,
    },
    Other,
}

/// A `user` message.
#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub parent: Option<String>,
    /// Its text, for a message of the person's or the CLI's.
    pub text: Option<String>,
    /// The CLI echoes a message it took (`--replay-user-messages`).
    pub replay: bool,
    /// The CLI wrote it itself, such as a compaction's summary.
    pub synthetic: bool,
    pub results: Vec<ToolResult>,
}

/// A tool's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub error: bool,
    pub text: String,
    /// The text was cut to [`MAX_TEXT`].
    pub cut: bool,
    /// How many lines the text has, counted once as it came.
    pub lines: usize,
    /// The hunks of a file change, from the message's `tool_use_result.structuredPatch`.
    pub patch: Vec<Hunk>,
    /// The hunks' added and removed lines, counted once as they came.
    pub changed: (usize, usize),
}

/// One hunk of a file change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    /// Each line with its first character: ` `, `-` or `+`.
    pub lines: Vec<String>,
}

/// What `result` says of a turn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnResult {
    /// `success`, `error_during_execution`, `error_max_turns` and the like.
    pub subtype: String,
    pub error: bool,
    /// Why it ended, such as `completed` or `aborted_streaming`.
    pub reason: Option<String>,
    pub millis: Option<u64>,
    pub cost_usd: Option<f64>,
    pub turns: Option<u64>,
    pub denials: usize,
}

/// What `rate_limit_event` says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateLimit {
    pub status: String,
    pub kind: String,
    /// When the window resets, in seconds since the epoch.
    pub resets_at: Option<u64>,
    /// How much of the window is used, from 0 to 1, where it says.
    pub utilization: Option<f64>,
}

/// The line that sends `text` as the person's message, ending in `\n`.
pub fn user_message(text: &str) -> String {
    let message = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    });
    let mut line = message.to_string();
    line.push('\n');
    line
}

/// What `line`, one line of the stream without its `\n`, says.
pub fn parse(line: &[u8]) -> Event {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(line) else {
        return Event::Unreadable { bytes: line.len() };
    };
    let kind = string(&object, "type");
    match kind.as_str() {
        "system" => system(&object),
        "stream_event" => partial(&object).map_or_else(|| other(&object), Event::Partial),
        "assistant" => assistant(&object).map_or_else(|| other(&object), Event::Assistant),
        "user" => user(&object).map_or_else(|| other(&object), Event::User),
        "result" => Event::Result(result(&object)),
        "rate_limit_event" => Event::RateLimit(rate_limit(&object)),
        _ => other(&object),
    }
}

fn other(object: &Map<String, Value>) -> Event {
    let kind = string(object, "type");
    let subtype = string(object, "subtype");
    Event::Other(if subtype.is_empty() {
        kind
    } else {
        format!("{kind}/{subtype}")
    })
}

fn system(object: &Map<String, Value>) -> Event {
    match string(object, "subtype").as_str() {
        "init" => Event::Init(Init {
            session_id: string(object, "session_id"),
            cwd: text(object, "cwd", MAX_TEXT).0,
            model: string(object, "model"),
            permission_mode: string(object, "permissionMode"),
            version: string(object, "claude_code_version"),
            capabilities: object
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .take(256)
                        .map(name)
                        .collect()
                })
                .unwrap_or_default(),
        }),
        "status" => Event::Status(match object.get("status").and_then(Value::as_str) {
            Some("requesting") => Status::Requesting,
            Some("compacting") => Status::Compacting,
            _ => match object.get("compact_result").and_then(Value::as_str) {
                Some(outcome) => Status::Compacted {
                    ok: outcome == "success",
                },
                None => Status::Other,
            },
        }),
        "compact_boundary" => {
            let meta = object.get("compact_metadata").and_then(Value::as_object);
            let number = |key| meta.and_then(|meta| meta.get(key)).and_then(Value::as_u64);
            Event::Compacted {
                pre_tokens: number("pre_tokens"),
                post_tokens: number("post_tokens"),
                manual: meta.is_some_and(|meta| string(meta, "trigger") == "manual"),
            }
        }
        "permission_denied" => Event::Denied {
            tool_use_id: string(object, "tool_use_id"),
            message: text(object, "message", MAX_TEXT).0,
        },
        "task_started" | "task_progress" | "task_updated" | "task_notification" => {
            let status = object
                .get("patch")
                .and_then(Value::as_object)
                .map_or_else(|| string(object, "status"), |patch| string(patch, "status"));
            Event::Task(Task {
                tool_use_id: Some(string(object, "tool_use_id")).filter(|id| !id.is_empty()),
                description: Some(text(object, "description", MAX_NAME * 4).0)
                    .filter(|text| !text.is_empty()),
                status: Some(status).filter(|status| !status.is_empty()),
            })
        }
        _ => other(object),
    }
}

/// A short string field, cut to [`MAX_NAME`]; empty when absent or not a string.
pub(crate) fn string(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(name)
        .unwrap_or_default()
}

pub(crate) fn name(text: &str) -> String {
    capped(text, MAX_NAME).0.to_owned()
}

/// A text field cut to `max`, and whether it was cut.
pub(crate) fn text(object: &Map<String, Value>, key: &str, max: usize) -> (String, bool) {
    let (text, cut) = capped(object.get(key).and_then(Value::as_str).unwrap_or(""), max);
    (text.to_owned(), cut)
}
