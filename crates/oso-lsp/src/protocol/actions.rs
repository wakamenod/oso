//! Code actions, as plain values (ADR 0138, step 13): what an ask for them sends, the range and
//! the diagnostics there as the server gave them, what `initialize` says of them, and what an
//! answer and a resolve give, read within the bounds, with their workspace edits.
//!
//! Must not depend on: `server`, threads or processes; this builds and reads values, and the
//! session thread judges the paths and places the ranges against a text.
//!
//! - An answer keeps at most [`CODE_ACTIONS_MAX`] actions, in its order, counting the rest. A
//!   title or a kind is cut at [`ACTION_TITLE_BYTES_MAX`], its controls written as spaces, as
//!   a list shows it on one line.
//! - A workspace edit is read whole or not at all (Q7): within [`EDIT_FILES_MAX`] files,
//!   [`EDIT_EDITS_MAX`] edits and [`EDIT_BYTES_MAX`] of new text, none with a control character
//!   other than a tab or a line feed, as a part of it could leave the text broken. One that
//!   creates, renames or deletes a file is refused (Q6): oso claims none of those.
//! - `Debug` gives counts and titles only: an edit's text may quote what the server read.

use oso_core::diagnostic::Severity;
use serde_json::{Map, Value, json};

use super::Extra;
use super::published::{Reported, range_of};
use crate::json::{array, integer, string};
use crate::limits::{
    ACTION_TITLE_BYTES_MAX, CODE_ACTIONS_MAX, EDIT_BYTES_MAX, EDIT_EDITS_MAX, EDIT_FILES_MAX,
    URI_BYTES_MAX,
};
use crate::position::Position;

/// What `initialize`'s result says of code actions: whether the server offers them, and
/// whether it resolves one's edit once picked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Acting {
    pub offers: bool,
    pub resolves: bool,
}

/// What `initialize`'s result says of code actions: none from a server that says nothing.
pub fn acting(result: &Value) -> Acting {
    let provider = (result.get("capabilities")).and_then(|caps| caps.get("codeActionProvider"));
    match provider {
        Some(Value::Bool(offers)) => Acting {
            offers: *offers,
            resolves: false,
        },
        Some(provider @ Value::Object(_)) => Acting {
            offers: true,
            resolves: provider.get("resolveProvider") == Some(&Value::Bool(true)),
        },
        _ => Acting::default(),
    }
}

/// `textDocument/codeAction`'s parameters: the range from `start` to `end` of the document at
/// `uri`, and `diagnostics`, those there, as the server gave them, every kind of action asked
/// for, as the person asked (Q1): nothing asks by itself (Q10).
pub fn code_action_params(
    uri: &str,
    (start, end): (Position, Position),
    diagnostics: &[&Reported],
) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "range": range_json(start, end),
        "context": {
            "diagnostics": diagnostics.iter().map(|reported| diagnostic_json(reported)).collect::<Vec<_>>(),
            "triggerKind": 1,
        },
    })
}

fn position_json(at: Position) -> Value {
    json!({"line": at.line, "character": at.character})
}

fn range_json(start: Position, end: Position) -> Value {
    json!({"start": position_json(start), "end": position_json(end)})
}

/// A diagnostic as the server gave it, as far as it was kept: its range, severity, code, of the
/// type it had, source, message, tags and `data`.
fn diagnostic_json(reported: &Reported) -> Value {
    let mut fields = Map::new();
    fields.insert("range".into(), range_json(reported.start, reported.end));
    let severity = match reported.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Information => 3,
        Severity::Hint => 4,
    };
    fields.insert("severity".into(), json!(severity));
    if !reported.code.is_empty() {
        let number = (reported.code_number)
            .then(|| reported.code.parse::<i64>().ok())
            .flatten();
        let code = number.map_or_else(|| json!(reported.code), |number| json!(number));
        fields.insert("code".into(), code);
    }
    if !reported.source.is_empty() {
        fields.insert("source".into(), json!(reported.source));
    }
    fields.insert("message".into(), json!(reported.message));
    let tags: Vec<u8> = [
        (reported.tags.unnecessary, 1),
        (reported.tags.deprecated, 2),
    ]
    .into_iter()
    .filter_map(|(has, tag)| has.then_some(tag))
    .collect();
    if !tags.is_empty() {
        fields.insert("tags".into(), json!(tags));
    }
    if let Some(data) = (reported.data.as_deref()).and_then(|data| serde_json::from_str(data).ok())
    {
        fields.insert("data".into(), data);
    }
    Value::Object(fields)
}

/// The code actions an answer offers, at most [`CODE_ACTIONS_MAX`], and how many more it had,
/// with the place in the answer of each kept, for its resolve.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionList {
    pub actions: Vec<OfferedAction>,
    pub left: usize,
    pub at: Vec<usize>,
}

/// One code action, as the answer gives it: its title and kind, cut; whether the server
/// prefers it; why it cannot be applied, if the server says; its edit, if it has one, or why
/// the edit cannot be read; the command it runs, by name, if any; and whether a resolve may
/// give it an edit: a `CodeAction` with none yet, never a bare `Command`.
#[derive(Clone, PartialEq, Eq)]
pub struct OfferedAction {
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
    pub disabled: Option<String>,
    pub edit: Option<Result<WireEdit, &'static str>>,
    pub command: Option<String>,
    pub resolvable: bool,
}

impl std::fmt::Debug for OfferedAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfferedAction")
            .field("title", &self.title)
            .field("kind", &self.kind)
            .field("preferred", &self.preferred)
            .field("disabled", &self.disabled.is_some())
            .field("edit", &self.edit)
            .field("command", &self.command)
            .field("resolvable", &self.resolvable)
            .finish()
    }
}

/// A workspace edit as the server counts it: each file's edits, by URI, not yet judged, with
/// the version of the document it was made for, if it names one.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct WireEdit {
    pub files: Vec<WireFile>,
}

/// One file's edits of a [`WireEdit`].
#[derive(Clone, PartialEq, Eq)]
pub struct WireFile {
    pub uri: String,
    pub version: Option<i32>,
    pub edits: Vec<Extra>,
}

impl std::fmt::Debug for WireEdit {
    /// The counts only: an edit's text may quote what the server read.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let edits: usize = self.files.iter().map(|file| file.edits.len()).sum();
        f.debug_struct("WireEdit")
            .field("files", &self.files.len())
            .field("edits", &edits)
            .finish()
    }
}

/// The code actions `result` offers: a list of `CodeAction`s and `Command`s, or `null`.
pub fn code_actions(result: &Value) -> ActionList {
    let Some(given) = result.as_array() else {
        return ActionList::default();
    };
    let (at, actions) = (given.iter().take(CODE_ACTIONS_MAX).enumerate())
        .filter_map(|(at, value)| Some((at, offered(value)?)))
        .unzip();
    ActionList {
        left: given.len() - given.len().min(CODE_ACTIONS_MAX),
        actions,
        at,
    }
}

/// The actions of `result`, an answer, at `at`, taken out of it as the server gave them, for
/// their resolve.
pub fn take_actions(result: Value, at: &[usize]) -> Vec<Value> {
    let Value::Array(mut given) = result else {
        return Vec::new();
    };
    at.iter()
        .map(|&at| given.get_mut(at).map(Value::take).unwrap_or_default())
        .collect()
}

/// The edit `result`, a resolved code action, gives, if it has one.
pub fn resolved_action(result: &Value) -> Option<Result<WireEdit, &'static str>> {
    result.get("edit").map(workspace_edit)
}

/// One action of a list: a `CodeAction`, or a bare `Command`, whose `command` is its name.
fn offered(value: &Value) -> Option<OfferedAction> {
    let title = shown(value.get("title")?.as_str()?);
    if let Some(name) = value.get("command").and_then(Value::as_str) {
        return Some(OfferedAction {
            title,
            kind: None,
            preferred: false,
            disabled: None,
            edit: None,
            command: Some(shown(name)),
            resolvable: false,
        });
    }
    let edit = value.get("edit").map(workspace_edit);
    let command = (value.get("command"))
        .and_then(|command| command.get("command"))
        .and_then(Value::as_str)
        .map(shown);
    Some(OfferedAction {
        title,
        kind: value.get("kind").and_then(Value::as_str).map(shown),
        preferred: value.get("isPreferred") == Some(&Value::Bool(true)),
        disabled: (value.get("disabled"))
            .map(|disabled| shown(disabled.get("reason").and_then(Value::as_str).unwrap_or(""))),
        resolvable: edit.is_none(),
        edit,
        command,
    })
}

/// `text` cut at [`ACTION_TITLE_BYTES_MAX`], at a character, its controls as spaces.
fn shown(text: &str) -> String {
    let kept = &text[..text.floor_char_boundary(ACTION_TITLE_BYTES_MAX)];
    kept.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A workspace edit's text edits, by file: its `documentChanges` if it has them, else its
/// `changes`, whole and within the bounds, or why not.
pub fn workspace_edit(value: &Value) -> Result<WireEdit, &'static str> {
    let mut files = Vec::new();
    let mut budget = Budget::default();
    if let Some(changes) = value.get("documentChanges") {
        let changes = array(changes, EDIT_FILES_MAX + 1).ok_or(UNREAD)?;
        for change in changes {
            if change.get("kind").is_some() {
                return Err("it creates, renames or deletes files, which oso does not do");
            }
            let document = change.get("textDocument").ok_or(UNREAD)?;
            let uri = uri_of(document.get("uri"))?;
            let version = document.get("version").and_then(integer);
            files.push(WireFile {
                uri,
                version,
                edits: text_edits(change.get("edits"), &mut budget)?,
            });
        }
    } else if let Some(changes) = value.get("changes") {
        let changes = changes.as_object().ok_or(UNREAD)?;
        for (uri, edits) in changes {
            files.push(WireFile {
                uri: uri_of(Some(&Value::String(uri.clone())))?,
                version: None,
                edits: text_edits(Some(edits), &mut budget)?,
            });
            if files.len() > EDIT_FILES_MAX {
                break;
            }
        }
    }
    if files.len() > EDIT_FILES_MAX {
        return Err("it changes too many files");
    }
    Ok(WireEdit { files })
}

/// Why an edit that does not read cannot be applied.
const UNREAD: &str = "its edit does not read";

/// What an edit has used of its bounds so far.
#[derive(Default)]
struct Budget {
    edits: usize,
    bytes: usize,
}

fn uri_of(value: Option<&Value>) -> Result<String, &'static str> {
    let uri = value.and_then(Value::as_str).ok_or(UNREAD)?;
    if uri.len() > URI_BYTES_MAX {
        return Err(UNREAD);
    }
    Ok(uri.to_owned())
}

/// A list of text edits, within what is left of `budget`.
fn text_edits(value: Option<&Value>, budget: &mut Budget) -> Result<Vec<Extra>, &'static str> {
    let given = value.and_then(Value::as_array).ok_or(UNREAD)?;
    budget.edits += given.len();
    if budget.edits > EDIT_EDITS_MAX {
        return Err("it makes too many edits");
    }
    let mut edits = Vec::with_capacity(given.len());
    for edit in given {
        let (start, end) = edit.get("range").and_then(range_of).ok_or(UNREAD)?;
        let text = edit
            .get("newText")
            .and_then(|text| string(text, usize::MAX))
            .ok_or(UNREAD)?;
        budget.bytes += text.len();
        if budget.bytes > EDIT_BYTES_MAX {
            return Err("it inserts too much text");
        }
        if text
            .chars()
            .any(|c| c.is_control() && c != '\t' && c != '\n')
        {
            return Err("its text holds a control character");
        }
        edits.push((start, end, text.to_owned()));
    }
    Ok(edits)
}

#[cfg(test)]
mod tests;
