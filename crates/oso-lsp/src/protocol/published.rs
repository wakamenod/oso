//! What `textDocument/publishDiagnostics` carries, read within the bounds (ADR 0138,
//! Diagnostics, step 6): the document's URI, the version it names, if any, and at most
//! [`DIAGNOSTICS_MAX`] diagnostics, the most severe kept, each with its range as the server
//! counts it, its severity, tags, source, code and message, cut to their bounds.
//!
//! Must not depend on: `server`, threads or processes; this reads values, and the session
//! thread judges the path and places the ranges against a text.

use oso_core::diagnostic::{Severity, Tags};
use serde_json::Value;

use crate::json::{array, integer, integer_in, string, uinteger};
use crate::limits::{
    DIAGNOSTIC_LABEL_BYTES_MAX, DIAGNOSTIC_MESSAGE_BYTES_MAX, DIAGNOSTICS_MAX, URI_BYTES_MAX,
};
use crate::position::Position;

/// The most tags read of one diagnostic: the protocol has two.
const TAGS_MAX: usize = 8;

/// A bytes' cost counted for each entry beside its strings, so that empty ones count too.
const ENTRY_COST: usize = 64;

/// One server's report of a document's diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// The document's URI, at most [`URI_BYTES_MAX`] bytes, not yet judged.
    pub uri: String,
    /// The version of the document it was made for, if it names one.
    pub version: Option<i32>,
    /// The diagnostics, the most severe kept, in no order.
    pub diagnostics: Vec<Reported>,
}

/// One diagnostic of a report, its range as the server counts it.
#[derive(Clone, PartialEq, Eq)]
pub struct Reported {
    pub start: Position,
    pub end: Position,
    pub severity: Severity,
    pub tags: Tags,
    pub source: String,
    pub code: String,
    pub message: String,
}

impl std::fmt::Debug for Reported {
    /// The range and severity only: a message can quote what the server read.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reported")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("severity", &self.severity)
            .finish_non_exhaustive()
    }
}

impl Published {
    /// What it costs to keep, for [`crate::limits::DIAGNOSTICS_BYTES_MAX`].
    pub fn bytes(&self) -> usize {
        self.uri.len()
            + self
                .diagnostics
                .iter()
                .map(|reported| {
                    ENTRY_COST
                        + reported.source.len()
                        + reported.code.len()
                        + reported.message.len()
                })
                .sum::<usize>()
    }
}

/// `publishDiagnostics`' parameters, or `None` if they have no URI or no list. A diagnostic
/// with no range that reads is left out; one with no severity is an error, as other clients
/// read it. Past [`DIAGNOSTICS_MAX`], the most severe are kept.
pub fn published(params: &Value) -> Option<Published> {
    let uri = params.get("uri")?.as_str()?;
    if uri.len() > URI_BYTES_MAX {
        return None;
    }
    let version = params.get("version").and_then(integer);
    let list = params.get("diagnostics")?.as_array()?;
    let mut diagnostics: Vec<Reported> = list.iter().filter_map(reported).collect();
    if diagnostics.len() > DIAGNOSTICS_MAX {
        diagnostics.select_nth_unstable_by_key(DIAGNOSTICS_MAX - 1, |reported| reported.severity);
        diagnostics.truncate(DIAGNOSTICS_MAX);
    }
    Some(Published {
        uri: uri.to_owned(),
        version,
        diagnostics,
    })
}

fn reported(value: &Value) -> Option<Reported> {
    let range = value.get("range")?;
    let position = |value: Option<&Value>| -> Option<Position> {
        let value = value?;
        Some(Position::new(
            uinteger(value.get("line")?)?,
            uinteger(value.get("character")?)?,
        ))
    };
    let (start, end) = (position(range.get("start"))?, position(range.get("end"))?);
    let severity = match value.get("severity").and_then(|v| integer_in(v, 1..=4)) {
        Some(2) => Severity::Warning,
        Some(3) => Severity::Information,
        Some(4) => Severity::Hint,
        _ => Severity::Error,
    };
    let mut tags = Tags::default();
    for tag in value
        .get("tags")
        .and_then(|tags| array(tags, TAGS_MAX))
        .unwrap_or_default()
    {
        match tag.as_i64() {
            Some(1) => tags.unnecessary = true,
            Some(2) => tags.deprecated = true,
            _ => {}
        }
    }
    let text = |key: &str, max: usize| {
        value
            .get(key)
            .and_then(|value| string(value, max))
            .unwrap_or_default()
            .to_owned()
    };
    let code = match value.get("code") {
        Some(code) if code.is_i64() || code.is_u64() => code.to_string(),
        Some(code) => string(code, DIAGNOSTIC_LABEL_BYTES_MAX)
            .unwrap_or_default()
            .to_owned(),
        None => String::new(),
    };
    Some(Reported {
        start,
        end,
        severity,
        tags,
        source: text("source", DIAGNOSTIC_LABEL_BYTES_MAX),
        code,
        message: text("message", DIAGNOSTIC_MESSAGE_BYTES_MAX),
    })
}

#[cfg(test)]
mod tests;
