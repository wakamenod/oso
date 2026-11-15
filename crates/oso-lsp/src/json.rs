//! Reading a message's JSON within bounds (ADR 0138): parsing through `serde_json::Value`
//! with its nesting limit on, and reads of a value that cut what a server sends to a size
//! (a string to its bytes, an array to its items) or refuse a number out of range.
//!
//! Must not depend on: `framing`, `uri` or `position`; this sees only bytes and values.

use std::fmt;
use std::ops::RangeInclusive;

use serde_json::Value;

use crate::limits::JSON_VALUES_MAX;

/// Why a message was not parsed. It says where or how much, never what the message held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonError {
    /// Not one JSON value within the nesting limit: the message breaks the protocol.
    NotJson {
        /// The line where parsing stopped, from 1.
        line: usize,
        /// The column where parsing stopped, from 1.
        column: usize,
    },
    /// More than [`JSON_VALUES_MAX`] values, so it was dropped unparsed, as a message over
    /// the size bound is.
    TooManyValues,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotJson { line, column } => write!(f, "not JSON at line {line}, column {column}"),
            Self::TooManyValues => write!(f, "more than {JSON_VALUES_MAX} JSON values"),
        }
    }
}

/// Parses a message's bytes, after counting its values in one pass (see [`values_within`]).
/// Nesting as deep as [`JSON_DEPTH_LIMIT`](crate::limits::JSON_DEPTH_LIMIT) is an error, as
/// `serde_json`'s own limit, which stays on, makes it.
///
/// # Errors
///
/// More than [`JSON_VALUES_MAX`] values, or bytes that are not one JSON value within the
/// nesting limit.
pub fn parse(bytes: &[u8]) -> Result<Value, JsonError> {
    if !values_within(bytes, JSON_VALUES_MAX) {
        return Err(JsonError::TooManyValues);
    }
    serde_json::from_slice(bytes).map_err(|error| JsonError::NotJson {
        line: error.line(),
        column: error.column(),
    })
}

/// Whether `bytes` hold at most `max` JSON values, counted as one plus each `[`, `{` and `,`
/// outside strings: an upper bound on what parsing them makes, objects' entries included.
/// Bytes that are not JSON count the same way, and fail to parse later. One pass, stopping
/// past `max`.
fn values_within(bytes: &[u8], max: usize) -> bool {
    let (mut values, mut in_string, mut escaped) = (1_usize, false, false);
    for &byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' | b',' => {
                values += 1;
                if values > max {
                    return false;
                }
            }
            _ => {}
        }
    }
    values <= max
}

/// `value` as a string cut to at most `max_bytes`, at a character's boundary, or `None` if it
/// is not a string.
pub fn string(value: &Value, max_bytes: usize) -> Option<&str> {
    let text = value.as_str()?;
    Some(&text[..text.floor_char_boundary(max_bytes)])
}

/// `value` as an array cut to at most `max_items`, or `None` if it is not an array.
pub fn array(value: &Value, max_items: usize) -> Option<&[Value]> {
    let items = value.as_array()?;
    Some(&items[..items.len().min(max_items)])
}

/// `value` as an integer in `range`, or `None` if it is not one: a fraction, a number out of
/// range or anything else.
pub fn integer_in(value: &Value, range: RangeInclusive<i64>) -> Option<i64> {
    value.as_i64().filter(|number| range.contains(number))
}

/// `value` as the protocol's `uinteger`, 0 to 2³¹ − 1, or `None`.
pub fn uinteger(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|&number| number <= UINTEGER_MAX)
        .and_then(|number| u32::try_from(number).ok())
}

/// The largest `uinteger` the protocol allows.
const UINTEGER_MAX: u64 = (1 << 31) - 1;

/// `value` as the protocol's `integer`, −2³¹ to 2³¹ − 1, or `None`.
pub fn integer(value: &Value) -> Option<i32> {
    value.as_i64().and_then(|number| i32::try_from(number).ok())
}

#[cfg(test)]
mod tests;
