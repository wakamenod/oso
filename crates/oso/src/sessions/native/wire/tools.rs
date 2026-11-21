//! A tool call's input, read into what a native session shows of it (ADR 0165): the path, the
//! command or the pattern the call works on, by the tool's name; for a tool not known here,
//! its input as JSON, cut. Nothing past this file reads a tool's input as JSON.
//!
//! Must not depend on: `rows` or `render`; plain functions over a parsed value.
//!
//! - An input longer than [`MAX_INPUT`] as JSON, measured by a writer that stops at the limit,
//!   is still read for its fields, each cut, and says it was cut: a long command padded to
//!   hide its end still shows how it starts.

use serde_json::Value;

use super::{MAX_NAME, capped};

/// The longest input kept whole, as JSON, in bytes.
pub const MAX_INPUT: usize = 64 << 10;
/// The most of a command, a pattern or a query kept, in bytes.
pub const MAX_FIELD: usize = 16 << 10;
/// The most of a path kept, in bytes.
pub const MAX_PATH: usize = 4096;
/// The most of an unknown tool's input kept, as JSON, in bytes.
pub const MAX_DETAIL: usize = 2048;

/// What a tool call works on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolInput {
    /// The file it reads or changes, or the directory it searches.
    pub path: Option<String>,
    /// Its command, pattern, URL, query, skill or description.
    pub text: Option<String>,
    /// A subagent's type.
    pub kind: Option<String>,
    /// How many to-do items it writes.
    pub items: Option<usize>,
    /// An unknown tool's input as JSON, cut to [`MAX_DETAIL`].
    pub detail: Option<String>,
    /// The input was longer than [`MAX_INPUT`] as JSON.
    pub cut: bool,
}

/// What the input `value` of a call of `name` works on.
pub fn decode(name: &str, value: Option<&Value>) -> ToolInput {
    let Some(value) = value else {
        return ToolInput::default();
    };
    let field = |key: &str, max: usize| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(|text| capped(text, max).0.to_owned())
    };
    let mut input = ToolInput {
        cut: serde_json::to_writer(Counter(MAX_INPUT), value).is_err(),
        ..ToolInput::default()
    };
    match name {
        "Read" | "Edit" | "MultiEdit" | "Write" => input.path = field("file_path", MAX_PATH),
        "NotebookEdit" => input.path = field("notebook_path", MAX_PATH),
        "Bash" => input.text = field("command", MAX_FIELD),
        "Grep" => {
            input.text = field("pattern", MAX_FIELD);
            input.path = field("path", MAX_PATH);
        }
        "Glob" => {
            input.text = field("pattern", MAX_FIELD);
            input.path = field("path", MAX_PATH);
        }
        "WebFetch" => input.text = field("url", MAX_FIELD),
        "WebSearch" => input.text = field("query", MAX_FIELD),
        "Skill" => input.text = field("skill", MAX_NAME).or_else(|| field("command", MAX_NAME)),
        "Agent" | "Task" => {
            input.kind = field("subagent_type", MAX_NAME);
            input.text = field("description", MAX_FIELD);
        }
        "TodoWrite" => {
            input.items = value.get("todos").and_then(Value::as_array).map(Vec::len);
        }
        _ => {
            input.text = value
                .as_object()
                .and_then(|object| object.values().find_map(Value::as_str))
                .map(|text| capped(text, MAX_FIELD).0.to_owned());
            input.detail = (!value.is_null()).then(|| json_cut(value, MAX_DETAIL));
        }
    }
    input
}

/// A writer that keeps nothing and fails once more than its bytes were written.
struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or(std::io::ErrorKind::FileTooLarge)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `value` as JSON, cut to at most `max` bytes at a character, never written whole.
fn json_cut(value: &Value, max: usize) -> String {
    struct Kept(Vec<u8>, usize);
    impl std::io::Write for Kept {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let room = self.1 - self.0.len();
            self.0.extend_from_slice(&bytes[..bytes.len().min(room)]);
            if bytes.len() > room {
                return Err(std::io::ErrorKind::FileTooLarge.into());
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut kept = Kept(Vec::new(), max);
    let _ = serde_json::to_writer(&mut kept, value);
    let valid = std::str::from_utf8(&kept.0).map_or_else(|error| error.valid_up_to(), str::len);
    String::from_utf8_lossy(&kept.0[..valid]).into_owned()
}
