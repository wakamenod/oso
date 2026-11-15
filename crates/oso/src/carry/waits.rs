//! What waits in a buffer for the person, in a restart's state (ADR 0118, Q8): an agent's open
//! and save marks, by kind, written only when something waits and read back by kind.
//!
//! Must not depend on: `app` or the editor; this turns a buffer's marks into fields and back.

use oso_core::buffer::{Buffer, Start, Starts};
use serde_json::{Map, Value};

/// The fields the marks go in.
const OPEN: &str = "waits_open";
const SAVE: &str = "waits_save";

/// Writes `buffer`'s marks into its `entry`, each only when something waits there.
pub(super) fn export(buffer: &Buffer, entry: &mut Value) {
    for (name, waits) in [(OPEN, buffer.open_waits()), (SAVE, buffer.save_waits())] {
        if !waits.is_empty() {
            entry[name] = waits.iter().map(Start::id).collect();
        }
    }
}

/// The open and the save marks of a buffer's `entry`.
pub(super) fn parse(entry: &Map<String, Value>) -> Result<(Starts, Starts), String> {
    Ok((kinds(entry, OPEN)?, kinds(entry, SAVE)?))
}

/// The kinds under `name`. A kind this build does not know is left out: it starts nothing here.
fn kinds(entry: &Map<String, Value>, name: &str) -> Result<Starts, String> {
    match entry.get(name) {
        None => Ok(Starts::NONE),
        Some(Value::Array(kinds)) if kinds.len() <= Start::ALL.len() * 4 => {
            kinds.iter().try_fold(Starts::NONE, |waits, kind| {
                let kind = kind
                    .as_str()
                    .ok_or(format!("a buffer's {name} holds a non-string"))?;
                Ok(Start::from_id(kind).map_or(waits, |start| waits.with(start)))
            })
        }
        Some(_) => Err(format!("a buffer's {name} is not a short list")),
    }
}
