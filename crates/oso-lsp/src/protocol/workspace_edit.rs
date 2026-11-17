//! A workspace edit as a server sends it (ADR 0138, step 13): each file's text edits, by URI,
//! read whole or not at all, for a code action's edit, and later a server's own
//! `workspace/applyEdit` (13b) and a rename's (step 14).
//!
//! Must not depend on: `server`, threads or processes; this reads values, and the session
//! thread judges the paths and places the ranges against a text.
//!
//! - Its `documentChanges` if it has them, else its `changes`: at most [`EDIT_FILES_MAX`]
//!   files, [`EDIT_EDITS_MAX`] edits and [`EDIT_BYTES_MAX`] of new text, none with a control
//!   character other than a tab or a line feed, as a part of it could leave the text broken.
//! - One that creates, renames or deletes a file is refused (Q6): oso claims none of those.
//! - `Debug` gives counts only: an edit's text may quote what the server read.

use serde_json::Value;

use super::Extra;
use super::completion::has_controls;
use super::published::range_of;
use crate::json::{array, integer, string};
use crate::limits::{EDIT_BYTES_MAX, EDIT_EDITS_MAX, EDIT_FILES_MAX, URI_BYTES_MAX};

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
        if has_controls(text) {
            return Err("its text holds a control character");
        }
        edits.push((start, end, text.to_owned()));
    }
    Ok(edits)
}

#[cfg(test)]
mod tests;
