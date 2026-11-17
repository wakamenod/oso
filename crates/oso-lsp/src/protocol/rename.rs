//! Rename, as plain values (ADR 0138, step 14): what `initialize` says of it, what a
//! `textDocument/prepareRename` and a `textDocument/rename` send, and what a prepare's answer
//! gives. A rename's answer is a workspace edit (`workspace_edit`).
//!
//! Must not depend on: `server`, threads or processes; this builds and reads values, and the
//! session thread places the positions against a text.
//!
//! - A prepare's placeholder is cut at [`NEW_NAME_BYTES_MAX`], as a prompt's label shows it on
//!   one line: its controls as spaces, and its bidi and invisible characters as `\u{…}`.
//! - A new name is never empty, and at most [`NEW_NAME_BYTES_MAX`] with no control character
//!   ([`new_name_fits`]); the session refuses another.

use serde_json::{Value, json};

use super::published::range_of;
use super::shown;
use crate::limits::NEW_NAME_BYTES_MAX;
use crate::position::Position;

/// What `initialize`'s result says of rename: whether the server renames, and whether it checks
/// a place first (`prepareProvider`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Renaming {
    pub offers: bool,
    pub prepares: bool,
}

/// What `initialize`'s result says of rename: none from a server that says nothing.
pub fn renaming(result: &Value) -> Renaming {
    let provider = result
        .get("capabilities")
        .and_then(|caps| caps.get("renameProvider"));
    match provider {
        Some(Value::Bool(offers)) => Renaming {
            offers: *offers,
            prepares: false,
        },
        Some(provider @ Value::Object(_)) => Renaming {
            offers: true,
            prepares: provider.get("prepareProvider") == Some(&Value::Bool(true)),
        },
        _ => Renaming::default(),
    }
}

/// `textDocument/prepareRename`'s parameters: the place, as a definition's are.
pub fn prepare_rename_params(uri: &str, position: Position) -> Value {
    super::position_params(uri, position)
}

/// `textDocument/rename`'s parameters: the place and the new name.
pub fn rename_params(uri: &str, position: Position, name: &str) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": {"line": position.line, "character": position.character},
        "newName": name,
    })
}

/// Whether `name` may be sent as a new name: not empty, at most [`NEW_NAME_BYTES_MAX`], with no
/// control character.
pub(crate) fn new_name_fits(name: &str) -> bool {
    !name.is_empty() && name.len() <= NEW_NAME_BYTES_MAX && !name.chars().any(char::is_control)
}

/// What a prepare's answer gives: `None` for nothing to rename there, as `null` says; `Some(None)`
/// for the server's own way, as `defaultBehavior` says, which leaves the identifier at point; or
/// the range it would rename, and its placeholder, shown on one line.
pub fn prepared(result: &Value) -> Option<Option<(Position, Position, Option<String>)>> {
    match result {
        Value::Object(fields) if fields.contains_key("defaultBehavior") => Some(None),
        // A `Range`, or one with its `placeholder`.
        Value::Object(fields) => {
            let (start, end) = match fields.get("range") {
                Some(range) => range_of(range)?,
                None => range_of(result)?,
            };
            let placeholder = (fields.get("placeholder"))
                .and_then(Value::as_str)
                .map(|text| shown(&text[..text.floor_char_boundary(NEW_NAME_BYTES_MAX)]));
            Some(Some((start, end, placeholder)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
