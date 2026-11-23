//! One line of Claude Code's transcript of a conversation, `<projects>/<slug>/<id>.jsonl`, read
//! for a native session's earlier turns (ADR 0165, step 5): first what places it in the
//! conversation, read without decoding, then, for the lines shown, what it says.
//!
//! Must not depend on: files, threads or the UI; plain functions over a line's bytes.
//!
//! - A line of the conversation starts `{"parentUuid":`; any other is the CLI's own record,
//!   such as the account a session ran as, and is never read further. Of a line of the
//!   conversation, only its top-level fields are read first, by their brackets and quotes:
//!   an attachment, which may hold the account's details or a system prompt, is placed in
//!   the conversation and never decoded.
//! - A user or assistant line's `message` is the stream's, read by `messages` as the stream's
//!   is: the transcript's `toolUseResult` and `thinkingDurationMs` are the stream's
//!   `tool_use_result` and `thinking_duration_ms`.
//! - A message is the person's only where the transcript marks it so: sent by oso or typed in
//!   the TUI (its `promptSource`), not the CLI's own (`isMeta`), and from a person (its
//!   `origin`, where it says one). A message from another session is a note; anything else a
//!   user line says is what the stream's replay of it would show.

use std::ops::Range;
use std::path::PathBuf;

use serde_json::{Map, Value};

use super::messages::{assistant, user};
use super::{Assistant, User, string};
use crate::sessions::history::scan::fields;
use crate::sessions::transcripts::MAX_CWD;

/// How every line of the conversation starts.
pub const MESSAGE_START: &[u8] = b"{\"parentUuid\":";

/// The longest id read, in bytes; a longer one is none.
pub const MAX_ID: usize = 64;

/// The prompt sources of a message the person sent: through oso, or typed in the TUI.
const PERSON: [&str; 4] = ["sdk", "typed", "queued", "suggestion_accepted"];

/// The transcript's names for fields the stream names otherwise.
const RENAMED: [(&str, &str); 2] = [
    ("toolUseResult", "tool_use_result"),
    ("thinkingDurationMs", "thinking_duration_ms"),
];

/// What a line of the conversation shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    /// A message the person sent.
    Prompt,
    /// A message from another session.
    Peer,
    /// Any other user or assistant message.
    Message,
    /// The conversation was compacted.
    Compacted,
    /// Nothing: an attachment, a compaction's summary or another system line.
    Nothing,
}

/// Where a line is in the conversation, and what it shows, read without decoding it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub uuid: String,
    /// The line it follows: its parent, or across a compaction the line before it.
    pub parent: Option<String>,
    pub shows: Shows,
}

/// What a line shown says.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// A message the person sent: its text and its line's id.
    Prompt {
        text: String,
        uuid: Option<String>,
    },
    /// A message from another session: its text and its line's id.
    Note {
        text: String,
        uuid: Option<String>,
    },
    /// Tool results, or a message that is not the person's.
    User(User),
    Assistant(Assistant),
    Compacted {
        pre: Option<u64>,
        post: Option<u64>,
        uuid: Option<String>,
    },
}

/// A line that is not one JSON object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// Where `line`, one line of a transcript without its `\n`, is in the conversation: `None`
/// for a line that is not of the conversation, has no id, or is of a side chain.
///
/// # Errors
///
/// A line of the conversation whose fields do not read, such as one nested too deep.
pub fn head(line: &[u8]) -> Result<Option<Head>, Unreadable> {
    if !line.starts_with(MESSAGE_START) {
        return Ok(None);
    }
    let [
        kind,
        subtype,
        uuid,
        parent,
        logical,
        side,
        meta,
        compact,
        source,
        turn,
        origin,
    ] = fields(
        line,
        [
            "type",
            "subtype",
            "uuid",
            "parentUuid",
            "logicalParentUuid",
            "isSidechain",
            "isMeta",
            "isCompactSummary",
            "promptSource",
            "turnOrigin",
            "origin",
        ],
    );
    let value = |range: Option<Range<usize>>| range.map(|range| &line[range]);
    let yes = |range| value(range) == Some(b"true".as_slice());
    // Every line of the conversation has a type: none read means none of its fields did.
    if kind.is_none() {
        return Err(Unreadable);
    }
    let Some(uuid) = value(uuid).and_then(short) else {
        return Ok(None);
    };
    if yes(side) {
        return Ok(None);
    }
    let shows = match value(kind) {
        Some(b"\"assistant\"") => Shows::Message,
        Some(b"\"system\"") => match value(subtype).and_then(short).as_deref() {
            Some("compact_boundary") => Shows::Compacted,
            _ => Shows::Nothing,
        },
        Some(b"\"user\"") if yes(compact) => Shows::Nothing,
        Some(b"\"user\"") => {
            let origin = value(origin)
                .and_then(|origin| value_of(origin, "kind"))
                .and_then(short);
            let turn = value(turn).and_then(short);
            let source = value(source).and_then(short);
            if turn.as_deref() == Some("peer") || origin.as_deref() == Some("peer") {
                Shows::Peer
            } else if !yes(meta)
                && source.is_some_and(|source| PERSON.contains(&source.as_str()))
                && origin.as_deref().is_none_or(|origin| origin == "human")
            {
                Shows::Prompt
            } else {
                Shows::Message
            }
        }
        _ => Shows::Nothing,
    };
    let parent = value(parent)
        .and_then(short)
        .or_else(|| value(logical).and_then(short));
    Ok(Some(Head {
        uuid,
        parent,
        shows,
    }))
}

/// The directory a line of the conversation says it ran in, if it says one.
pub fn cwd(line: &[u8]) -> Option<PathBuf> {
    if !line.starts_with(MESSAGE_START) {
        return None;
    }
    let [cwd] = fields(line, ["cwd"]);
    let cwd = &line[cwd?];
    if cwd.len() > MAX_CWD + 2 {
        return None;
    }
    serde_json::from_slice::<String>(cwd)
        .ok()
        .map(PathBuf::from)
}

/// What `line`, placed by [`head`] as showing `shows`, says: `None` when it shows nothing.
///
/// # Errors
///
/// A line that is not one JSON object.
pub fn said(line: &[u8], shows: Shows) -> Result<Option<Said>, Unreadable> {
    // Never decoded: an attachment may hold the account's details.
    if shows == Shows::Nothing {
        return Ok(None);
    }
    let Ok(Value::Object(mut object)) = serde_json::from_slice::<Value>(line) else {
        return Err(Unreadable);
    };
    for (from, to) in RENAMED {
        if let Some(value) = object.remove(from) {
            object.insert(to.to_owned(), value);
        }
    }
    Ok(match shows {
        Shows::Compacted => Some(compacted(&object)),
        Shows::Message if string(&object, "type") == "assistant" => {
            assistant(&object).map(Said::Assistant)
        }
        Shows::Message => user(&object).map(Said::User),
        Shows::Prompt => user(&object)
            .and_then(|user| Some((user.text?, user.uuid)))
            .map(|(text, uuid)| Said::Prompt { text, uuid }),
        Shows::Peer => user(&object)
            .and_then(|user| Some((user.text?, user.uuid)))
            .map(|(text, uuid)| Said::Note { text, uuid }),
        Shows::Nothing => None,
    })
}

fn compacted(object: &Map<String, Value>) -> Said {
    let meta = object.get("compactMetadata").and_then(Value::as_object);
    let number = |key| meta.and_then(|meta| meta.get(key)).and_then(Value::as_u64);
    Said::Compacted {
        pre: number("preTokens"),
        post: number("postTokens"),
        uuid: Some(string(object, "uuid")).filter(|id| !id.is_empty()),
    }
}

/// The value of `key` at the top level of the JSON object `object`, by its bytes.
fn value_of<'a>(object: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let [range] = fields(object, [key]);
    range.map(|range| &object[range])
}

/// The string a short JSON string's bytes, quotes included, hold: none past [`MAX_ID`] or
/// when empty.
fn short(value: &[u8]) -> Option<String> {
    if value.first() != Some(&b'"') || value.len() > MAX_ID + 2 {
        return None;
    }
    serde_json::from_slice::<String>(value)
        .ok()
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests;
