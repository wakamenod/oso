//! What `textDocument/definition` and `textDocument/references` answer, read within the bounds
//! (ADR 0138, Go to definition and back, step 8): a `Location`, a list of them, or a list of
//! `LocationLink`s, whose `targetSelectionRange` is the place, as other clients read it. At most
//! [`LOCATIONS_MAX`] are kept, in the order they came.
//!
//! Must not depend on: `server`, threads or processes; this reads values, and the session
//! thread judges the paths and places the ranges against a text.

use serde_json::{Value, json};

use crate::json::string;
use crate::limits::{LOCATIONS_MAX, URI_BYTES_MAX};
use crate::position::Position;

use super::published::range_of;

/// One place an answer names, its range as the server counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// The document's URI, at most [`URI_BYTES_MAX`] bytes, not yet judged.
    pub uri: String,
    pub start: Position,
    pub end: Position,
}

/// The places an answer names, and how many of those it named were left out: past
/// [`LOCATIONS_MAX`], or not readable. `null` names none.
pub fn located(result: &Value) -> (Vec<Located>, usize) {
    let items = match result {
        Value::Array(items) => items.as_slice(),
        Value::Null => return (Vec::new(), 0),
        single => std::slice::from_ref(single),
    };
    let kept = &items[..items.len().min(LOCATIONS_MAX)];
    let mut places = Vec::with_capacity(kept.len());
    places.extend(kept.iter().filter_map(one));
    let left = items.len() - places.len();
    (places, left)
}

/// A `Location`, or a `LocationLink` by its target.
fn one(value: &Value) -> Option<Located> {
    let (uri, range) = match value.get("targetUri") {
        Some(uri) => (
            uri,
            value
                .get("targetSelectionRange")
                .or_else(|| value.get("targetRange"))?,
        ),
        None => (value.get("uri")?, value.get("range")?),
    };
    let uri = string(uri, URI_BYTES_MAX + 1).filter(|uri| uri.len() <= URI_BYTES_MAX)?;
    let (start, end) = range_of(range)?;
    Some(Located {
        uri: uri.to_owned(),
        start,
        end,
    })
}

/// The parameters of `textDocument/definition` at `position` in the document `uri`.
pub fn definition_params(uri: &str, position: Position) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": {"line": position.line, "character": position.character},
    })
}

/// The parameters of `textDocument/references`: as [`definition_params`], the declaration
/// included, as Eglot asks.
pub fn references_params(uri: &str, position: Position) -> Value {
    let mut params = definition_params(uri, position);
    params["context"] = json!({"includeDeclaration": true});
    params
}

#[cfg(test)]
mod tests;
