//! The bodies of a native session's stream lines (ADR 0165): a message's blocks as it streams
//! and whole, the person's and the tools' results, a turn's result and the rate limit.
//!
//! Must not depend on: `rows` or `render`; plain functions over a parsed line.

use serde_json::{Map, Value};

use super::tools::decode;
use super::{
    Assistant, Block, BlockKind, Hunk, MAX_DIFF_BYTES, MAX_DIFF_LINES, MAX_TEXT, PartKind, Partial,
    RateLimit, ToolResult, TurnResult, User, capped, name, string, text,
};

pub(super) fn partial(object: &Map<String, Value>) -> Option<Partial> {
    let event = object.get("event")?.as_object()?;
    let index = || event.get("index").and_then(Value::as_u64);
    let part = match string(event, "type").as_str() {
        "content_block_start" => {
            let block = event.get("content_block")?.as_object()?;
            let block = match string(block, "type").as_str() {
                "text" => BlockKind::Text,
                "thinking" | "redacted_thinking" => BlockKind::Thinking,
                "tool_use" | "server_tool_use" => BlockKind::ToolUse {
                    id: string(block, "id"),
                    name: string(block, "name"),
                },
                _ => BlockKind::Other,
            };
            PartKind::Start {
                index: index()?,
                block,
            }
        }
        "content_block_delta" => {
            let delta = event.get("delta")?.as_object()?;
            let key = match string(delta, "type").as_str() {
                "text_delta" => "text",
                "thinking_delta" => "thinking",
                "input_json_delta" => "partial_json",
                _ => {
                    return Some(Partial {
                        message_id: message_id(object),
                        parent: parent(object),
                        part: PartKind::Other,
                    });
                }
            };
            PartKind::Delta {
                index: index()?,
                text: text(delta, key, MAX_TEXT).0,
            }
        }
        "content_block_stop" => PartKind::Stop { index: index()? },
        _ => PartKind::Other,
    };
    Some(Partial {
        message_id: message_id(object),
        parent: parent(object),
        part,
    })
}

fn message_id(object: &Map<String, Value>) -> String {
    string(object, "api_message_id")
}

fn parent(object: &Map<String, Value>) -> Option<String> {
    Some(string(object, "parent_tool_use_id")).filter(|id| !id.is_empty())
}

pub(super) fn assistant(object: &Map<String, Value>) -> Option<Assistant> {
    let message = object.get("message")?.as_object()?;
    let millis = object.get("thinking_duration_ms").and_then(Value::as_u64);
    let blocks = message
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .take(1024)
        .map(|block| match string(block, "type").as_str() {
            "text" => Block::Text(text(block, "text", MAX_TEXT).0),
            "thinking" => Block::Thinking {
                text: text(block, "thinking", MAX_TEXT).0,
                millis,
            },
            "redacted_thinking" => Block::Thinking {
                text: String::new(),
                millis,
            },
            "tool_use" | "server_tool_use" => Block::ToolUse {
                id: string(block, "id"),
                name: string(block, "name"),
                input: decode(&string(block, "name"), block.get("input")),
            },
            _ => Block::Other,
        })
        .collect();
    Some(Assistant {
        message_id: string(message, "id"),
        parent: parent(object),
        model: super::whole(message, "model"),
        blocks,
        context_tokens: context(message.get("usage")),
    })
}

/// The tokens a request read, of its `usage`: what the context holds, as `/context` counts it
/// (measured, part 3a); `None` when it gives none of the three counts.
fn context(usage: Option<&Value>) -> Option<u64> {
    let usage = usage?.as_object()?;
    let counts = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ];
    let counts = counts.map(|key| usage.get(key).and_then(Value::as_u64));
    counts
        .iter()
        .any(Option::is_some)
        .then(|| counts.into_iter().flatten().fold(0, u64::saturating_add))
}

/// Each model's context window, of a `result`'s `modelUsage`.
fn windows(usage: Option<&Value>) -> Vec<(String, u64)> {
    let Some(usage) = usage.and_then(Value::as_object) else {
        return Vec::new();
    };
    usage
        .iter()
        .filter_map(|(model, used)| {
            let window = used.get("contextWindow").and_then(Value::as_u64)?;
            Some((super::name(model), window))
        })
        .take(super::MAX_WINDOWS)
        .collect()
}

pub(super) fn user(object: &Map<String, Value>) -> Option<User> {
    let message = object.get("message")?.as_object()?;
    let mut user = User {
        parent: parent(object),
        text: None,
        replay: object
            .get("isReplay")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        synthetic: object
            .get("isSynthetic")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || object
                .get("isMeta")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        results: Vec::new(),
        todo: None,
    };
    match message.get("content")? {
        Value::String(content) => user.text = Some(capped(content, MAX_TEXT).0.to_owned()),
        Value::Array(blocks) => {
            let patch = patch(object.get("tool_use_result"));
            let mut texts = Vec::new();
            for block in blocks.iter().filter_map(Value::as_object).take(1024) {
                match string(block, "type").as_str() {
                    "text" => texts.push(text(block, "text", MAX_TEXT).0),
                    "tool_result" => {
                        let (text, cut) = result_text(block.get("content"));
                        user.results.push(ToolResult {
                            tool_use_id: string(block, "tool_use_id"),
                            error: block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            lines: text.lines().count(),
                            text,
                            cut,
                            patch: Vec::new(),
                            changed: (0, 0),
                        });
                    }
                    _ => {}
                }
            }
            if let [only] = user.results.as_mut_slice() {
                only.changed = counts(&patch);
                only.patch = patch;
                user.todo = super::todo::done(object.get("tool_use_result"));
            }
            if !texts.is_empty() {
                user.text = Some(joined(texts.iter().map(String::as_str)).0);
            }
        }
        _ => return None,
    }
    Some(user)
}

/// A tool result's text: a string, or its `text` parts joined.
fn result_text(content: Option<&Value>) -> (String, bool) {
    match content {
        Some(Value::String(text)) => {
            let (text, cut) = capped(text, MAX_TEXT);
            (text.to_owned(), cut)
        }
        Some(Value::Array(parts)) => joined(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str)),
        ),
        _ => (String::new(), false),
    }
}

/// `texts` joined by newlines, cut to [`MAX_TEXT`], and whether it was cut. It stops adding
/// once past the limit.
fn joined<'a>(texts: impl Iterator<Item = &'a str>) -> (String, bool) {
    let mut joined = String::new();
    for text in texts {
        if !joined.is_empty() {
            joined.push('\n');
        }
        let (part, _) = capped(text, MAX_TEXT + 1 - joined.len().min(MAX_TEXT));
        joined.push_str(part);
        if joined.len() > MAX_TEXT {
            break;
        }
    }
    let (kept, cut) = capped(&joined, MAX_TEXT);
    let kept = kept.len();
    joined.truncate(kept);
    (joined, cut)
}

/// The hunks' added and removed lines.
pub fn counts(patch: &[Hunk]) -> (usize, usize) {
    patch
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .fold((0, 0), |(added, removed), line| {
            match line.as_bytes().first() {
                Some(b'+') => (added + 1, removed),
                Some(b'-') => (added, removed + 1),
                _ => (added, removed),
            }
        })
}

/// The hunks of `tool_use_result.structuredPatch`, at most [`MAX_DIFF_LINES`] lines and
/// [`MAX_DIFF_BYTES`] bytes in all.
fn patch(result: Option<&Value>) -> Vec<Hunk> {
    let Some(hunks) = result
        .and_then(Value::as_object)
        .and_then(|result| result.get("structuredPatch"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut left = MAX_DIFF_LINES;
    let mut bytes = MAX_DIFF_BYTES;
    let mut patch = Vec::new();
    for hunk in hunks.iter().filter_map(Value::as_object) {
        let number = |key| hunk.get(key).and_then(Value::as_u64).unwrap_or(0);
        let mut lines = Vec::new();
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let line = capped(line, MAX_TEXT / 64).0;
            if left == 0 || line.len() > bytes {
                left = 0;
                break;
            }
            left -= 1;
            bytes -= line.len();
            // One line each: a newline in a line would start a row of its own.
            lines.push(line.replace(['\n', '\r'], " "));
        }
        // An empty hunk counts as a line, so that no number of them is kept.
        if lines.is_empty() {
            left = left.saturating_sub(1);
        }
        patch.push(Hunk {
            old_start: number("oldStart"),
            old_lines: number("oldLines"),
            new_start: number("newStart"),
            new_lines: number("newLines"),
            lines,
        });
        if left == 0 {
            break;
        }
    }
    patch
}

pub(super) fn result(object: &Map<String, Value>) -> TurnResult {
    TurnResult {
        subtype: string(object, "subtype"),
        error: object
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        reason: Some(string(object, "terminal_reason")).filter(|reason| !reason.is_empty()),
        millis: object.get("duration_ms").and_then(Value::as_u64),
        cost_usd: object.get("total_cost_usd").and_then(Value::as_f64),
        turns: object.get("num_turns").and_then(Value::as_u64),
        denials: object
            .get("permission_denials")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        session_id: string(object, "session_id"),
        windows: windows(object.get("modelUsage")),
        local_command: Some(string(object, "local_command")).filter(|name| !name.is_empty()),
        reply: (object.get("result").and_then(Value::as_str))
            .and_then(|reply| reply.lines().next())
            .map(name)
            .unwrap_or_default(),
    }
}

pub(super) fn rate_limit(object: &Map<String, Value>) -> RateLimit {
    let Some(info) = object.get("rate_limit_info").and_then(Value::as_object) else {
        return RateLimit::default();
    };
    let kind = string(info, "rateLimitType");
    let utilization = info
        .get("unifiedWindows")
        .and_then(Value::as_object)
        .and_then(|windows| windows.get(&kind))
        .and_then(|window| window.get("utilization"))
        .and_then(Value::as_f64);
    RateLimit {
        status: string(info, "status"),
        resets_at: info.get("resetsAt").and_then(Value::as_u64),
        utilization,
        kind,
    }
}
