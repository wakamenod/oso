//! What a request's block shows of a call's input (ADR 0182), read whole from the input the
//! request carries: what runs, why, and every other field, in the order they are shown.
//!
//! Must not depend on: `rows`, `render` or the UI; plain functions over a parsed value.
//!
//! - All of it is Claude's text, untrusted, and none of it is cut: an input past
//!   [`MAX_INPUT`](super::tools::MAX_INPUT) as JSON, which no request carries, is not read at all.
//! - Every field is in exactly one part. One of a known name that does not hold what the tool's
//!   does (a command that is no text) is shown as any other field, as JSON.

use serde_json::{Map, Value};

use super::tools::{MAX_INPUT, fits, indented};

/// The most line breaks of a value that is indented, as JSON.
const MAX_INDENTED: usize = 400;

/// A part of what a call asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// What a shell runs.
    Command(String),
    /// The file it changes.
    File(String),
    /// Which of how many changes the parts after it are of, from 1.
    Nth(usize, usize),
    /// A text put in place of another.
    Change { old: String, new: String },
    /// A text written as a file's whole.
    Written(String),
    /// Why it runs, as Claude says.
    Why(String),
    /// Any other field: its name, and its value.
    Field(String, Shown),
    /// The parts after it are the call's again, after its changes'.
    Call,
}

/// A field's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// A text, as it is: one that could be read as another value is JSON.
    Text(String),
    /// Any other, as JSON, in one line or more.
    Json(String),
}

impl Shown {
    /// `value`, indented where it nests and that takes at most `room` bytes, which it takes: all a
    /// call's values take less together than its input would twice.
    fn of(value: &Value, room: &mut usize) -> Self {
        match value {
            // One that says what another value does is told from it.
            Value::String(text) if reads_as_json(text) => Self::Json(value.to_string()),
            Value::String(text) => Self::Text(text.clone()),
            Value::Array(_) | Value::Object(_) => {
                let many = indented(value, *room)
                    .filter(|many| many.matches('\n').count() <= MAX_INDENTED);
                *room -= many.as_ref().map_or(0, String::len);
                Self::Json(many.unwrap_or_else(|| value.to_string()))
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => Self::Json(value.to_string()),
        }
    }
}

/// Whether `text` could be read as another value, as none, or without the white space it has at an
/// end.
fn reads_as_json(text: &str) -> bool {
    text.trim().len() < text.len().max(1) && !text.contains('\n')
        || serde_json::from_str::<Value>(text).is_ok()
}

/// Whether the tool `tool` changes a file, by its name.
pub fn changes_a_file(tool: &str) -> bool {
    matches!(tool, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
}

/// What a call of `tool` with `input` asks for, in the order it is shown; `None` for an input too
/// long to be shown, or that has no fields to name.
pub fn parts(tool: &str, input: &Value) -> Option<Vec<Part>> {
    let input = input.as_object().filter(|_| fits(input, MAX_INPUT))?;
    let mut room = MAX_INPUT;
    let mut parts = Vec::new();
    // The names of the fields shown.
    let mut shown: Vec<&str> = Vec::new();
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    match tool {
        "Bash" => {
            if let Some(command) = text("command") {
                parts.push(Part::Command(command));
                shown.push("command");
            }
            if let Some(why) = text("description").filter(|why| !why.is_empty()) {
                parts.push(Part::Why(why));
                shown.push("description");
            }
        }
        "Edit" | "MultiEdit" | "Write" => {
            if let Some(file) = text("file_path") {
                parts.push(Part::File(file));
                shown.push("file_path");
            }
            let edits = input.get("edits").and_then(Value::as_array);
            match (tool, edits) {
                ("Write", _) => {
                    if let Some(written) = text("content") {
                        parts.push(Part::Written(written));
                        shown.push("content");
                    }
                }
                ("MultiEdit", Some(edits)) => {
                    // None at all, or one that says nothing, is shown as any field.
                    let edits: Option<Vec<_>> = (edits.iter())
                        .map(|edit| edit.as_object().filter(|edit| !edit.is_empty()))
                        .collect();
                    let edits = edits.unwrap_or_default();
                    shown.extend((!edits.is_empty()).then_some("edits"));
                    for (at, edit) in edits.iter().enumerate() {
                        if edits.len() > 1 {
                            parts.push(Part::Nth(at + 1, edits.len()));
                        }
                        let changed = change(edit, &mut parts, &mut room);
                        fields(edit, changed, &mut parts, &mut room);
                    }
                    if matches!(parts.last(), Some(Part::Field(..))) || edits.len() > 1 {
                        parts.push(Part::Call);
                    }
                }
                ("Edit", _) => shown.extend(change(input, &mut parts, &mut room)),
                _ => {}
            }
        }
        "NotebookEdit" => {
            if let Some(file) = text("notebook_path") {
                parts.push(Part::File(file));
                shown.push("notebook_path");
            }
        }
        _ => {}
    }
    fields(input, &shown, &mut parts, &mut room);
    parts.pop_if(|last| *last == Part::Call);
    Some(parts)
}

/// Adds the change that `edit` makes to `parts`, under whether it is made everywhere, and says
/// which fields that shows.
fn change(
    edit: &Map<String, Value>,
    parts: &mut Vec<Part>,
    room: &mut usize,
) -> &'static [&'static str] {
    let text = |key: &str| edit.get(key).and_then(Value::as_str).map(str::to_owned);
    let (Some(old), Some(new)) = (text("old_string"), text("new_string")) else {
        return &[];
    };
    if let Some(all) = edit.get("replace_all") {
        parts.push(Part::Field("replace_all".to_owned(), Shown::of(all, room)));
    }
    parts.push(Part::Change { old, new });
    &["old_string", "new_string", "replace_all"]
}

/// Adds every field of `input` but those `shown` to `parts`.
fn fields(input: &Map<String, Value>, shown: &[&str], parts: &mut Vec<Part>, room: &mut usize) {
    for (name, value) in input {
        if !shown.contains(&name.as_str()) {
            parts.push(Part::Field(name.clone(), Shown::of(value, room)));
        }
    }
}

#[cfg(test)]
mod tests;
