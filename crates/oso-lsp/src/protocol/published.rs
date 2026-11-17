//! What `textDocument/publishDiagnostics` carries, read within the bounds (ADR 0138,
//! Diagnostics, step 6): the document's URI, the version it names, if any, and at most
//! [`DIAGNOSTICS_MAX`] diagnostics, the most severe kept, each with its range as the server
//! counts it, its severity, tags, source, code and message, cut to their bounds, and what an ask
//! for code actions gives back of it as the server gave it: its `data`, and whether its code was
//! a number (step 13).
//!
//! Must not depend on: `server`, threads or processes; this reads values, and the session
//! thread judges the path and places the ranges against a text.
//!
//! The list is counted by severity first, with nothing allocated, so that only the entries kept
//! are read, and the list holds room for no more than them.

use oso_core::diagnostic::{Severity, Tags};
use serde_json::Value;

use crate::json::{array, integer, integer_in, string, uinteger};
use crate::limits::{
    DIAGNOSTIC_DATA_BYTES_MAX, DIAGNOSTIC_LABEL_BYTES_MAX, DIAGNOSTIC_MESSAGE_BYTES_MAX,
    DIAGNOSTICS_MAX, URI_BYTES_MAX,
};
use crate::position::Position;

/// The most tags read of one diagnostic: the protocol has two.
const TAGS_MAX: usize = 8;

/// One server's report of a document's diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// The document's URI, at most [`URI_BYTES_MAX`] bytes, not yet judged.
    pub uri: String,
    /// The version of the document it was made for, if it names one.
    pub version: Option<i32>,
    /// The diagnostics, the most severe kept, each severity's in the order they came.
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
    /// The code was a number, which goes back as one (step 13).
    pub code_number: bool,
    pub message: String,
    /// Its `data`, as JSON, at most [`DIAGNOSTIC_DATA_BYTES_MAX`]; given back with an ask for
    /// code actions, as a server may need it to offer a fix (step 13).
    pub data: Option<Box<str>>,
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
    /// What it costs to keep, for [`crate::limits::DIAGNOSTICS_BYTES_MAX`]: its list as
    /// allocated, and each string.
    pub(crate) fn bytes(&self) -> usize {
        let strings: usize = self
            .diagnostics
            .iter()
            .map(|reported| {
                reported.source.len()
                    + reported.code.len()
                    + reported.message.len()
                    + reported.data.as_ref().map_or(0, |data| data.len())
            })
            .sum();
        self.uri.len() + self.diagnostics.capacity() * std::mem::size_of::<Reported>() + strings
    }
}

/// `publishDiagnostics`' parameters, or `None` if they have no URI or no list. A diagnostic
/// with no range that reads is left out; one with no severity is an error, as other clients
/// read it. Past [`DIAGNOSTICS_MAX`], the most severe are kept, each severity's first ones.
pub fn published(params: &Value) -> Option<Published> {
    let uri = params.get("uri")?.as_str()?;
    if uri.len() > URI_BYTES_MAX {
        return None;
    }
    let version = params.get("version").and_then(integer);
    let list = params.get("diagnostics")?.as_array()?;
    let mut counts = [0usize; 4];
    for value in list {
        if let Some(severity) = placed(value) {
            counts[severity as usize] += 1;
        }
    }
    // How many of each severity are kept, the most severe first.
    let mut left = DIAGNOSTICS_MAX;
    let mut quotas = counts.map(|count| {
        let quota = count.min(left);
        left -= quota;
        quota
    });
    let mut diagnostics = Vec::with_capacity(quotas.iter().sum());
    for value in list {
        let Some(severity) = placed(value) else {
            continue;
        };
        let quota = &mut quotas[severity as usize];
        if *quota > 0 {
            *quota -= 1;
            diagnostics.extend(reported(value, severity));
        }
    }
    Some(Published {
        uri: uri.to_owned(),
        version,
        diagnostics,
    })
}

/// The severity of `value` if it has a range that reads, which a diagnostic must.
fn placed(value: &Value) -> Option<Severity> {
    range(value)?;
    Some(
        match value.get("severity").and_then(|v| integer_in(v, 1..=4)) {
            Some(2) => Severity::Warning,
            Some(3) => Severity::Information,
            Some(4) => Severity::Hint,
            _ => Severity::Error,
        },
    )
}

/// `value` as JSON, if it takes at most [`DIAGNOSTIC_DATA_BYTES_MAX`]: the writing stops
/// there, so that a huge one costs no more than the bound.
fn bounded_json(value: &Value) -> Option<Box<str>> {
    /// A writer that refuses past the bound.
    struct Bounded(Vec<u8>);
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len() + bytes.len() > DIAGNOSTIC_DATA_BYTES_MAX {
                return Err(std::io::ErrorKind::FileTooLarge.into());
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Bounded(Vec::new());
    serde_json::to_writer(&mut out, value).ok()?;
    String::from_utf8(out.0).ok().map(String::into_boxed_str)
}

/// The range under `value`'s `range`.
fn range(value: &Value) -> Option<(Position, Position)> {
    range_of(value.get("range")?)
}

/// A range's start and end, if both read.
pub(super) fn range_of(range: &Value) -> Option<(Position, Position)> {
    let position = |value: Option<&Value>| -> Option<Position> {
        let value = value?;
        Some(Position::new(
            uinteger(value.get("line")?)?,
            uinteger(value.get("character")?)?,
        ))
    };
    Some((position(range.get("start"))?, position(range.get("end"))?))
}

fn reported(value: &Value, severity: Severity) -> Option<Reported> {
    let (start, end) = range(value)?;
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
    let data = (value.get("data"))
        .filter(|data| !data.is_null())
        .and_then(bounded_json);
    Some(Reported {
        start,
        end,
        severity,
        tags,
        source: text("source", DIAGNOSTIC_LABEL_BYTES_MAX),
        code_number: (value.get("code")).is_some_and(|code| code.is_i64() || code.is_u64()),
        code,
        message: text("message", DIAGNOSTIC_MESSAGE_BYTES_MAX),
        data,
    })
}

#[cfg(test)]
mod tests;
