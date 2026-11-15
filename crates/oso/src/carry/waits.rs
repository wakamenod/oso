//! What waits in a buffer for the person, in a restart's state (ADR 0118, Q8): an agent's open
//! and save marks, by kind, and whether the person let an agent's open go, each written only
//! when it holds something and read back as it was.
//!
//! Must not depend on: `app` or the editor; this turns a buffer's marks into fields and back.

use oso_core::buffer::{Buffer, Start, Starts};
use serde_json::{Map, Value};

/// The fields the marks go in.
const OPEN: &str = "waits_open";
const SAVE: &str = "waits_save";
const LET_GO: &str = "open_let_go";

/// A buffer's marks as a restart carried them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Carried {
    pub open: Starts,
    pub save: Starts,
    /// The person let an agent's open go, so it counts as theirs.
    pub open_let_go: bool,
}

/// Writes `buffer`'s marks into its `entry`, each only when it holds something.
pub(super) fn export(buffer: &Buffer, entry: &mut Value) {
    for (name, waits) in [(OPEN, buffer.open_waits()), (SAVE, buffer.save_waits())] {
        if !waits.is_empty() {
            entry[name] = waits.iter().map(Start::id).collect();
        }
    }
    if buffer.open_let_go() {
        entry[LET_GO] = Value::Bool(true);
    }
}

/// The marks of a buffer's `entry`.
pub(super) fn parse(entry: &Map<String, Value>) -> Result<Carried, String> {
    let open_let_go = match entry.get(LET_GO) {
        None => false,
        Some(Value::Bool(let_go)) => *let_go,
        Some(_) => return Err(format!("a buffer's {LET_GO} is not true or false")),
    };
    Ok(Carried {
        open: kinds(entry, OPEN)?,
        save: kinds(entry, SAVE)?,
        open_let_go,
    })
}

/// The kinds under `name`. A kind this build does not know holds every kind that runs a
/// program, so that nothing the old instance held starts here unasked.
fn kinds(entry: &Map<String, Value>, name: &str) -> Result<Starts, String> {
    let every = Start::ALL
        .into_iter()
        .fold(Starts::NONE, Starts::with)
        .programs();
    match entry.get(name) {
        None => Ok(Starts::NONE),
        Some(Value::Array(kinds)) if kinds.len() <= Start::ALL.len() * 4 => {
            kinds.iter().try_fold(Starts::NONE, |waits, kind| {
                let kind = kind
                    .as_str()
                    .ok_or(format!("a buffer's {name} holds a non-string"))?;
                Ok(Start::from_id(kind)
                    .map_or(every, |start| waits.with(start))
                    .union(waits))
            })
        }
        Some(_) => Err(format!("a buffer's {name} is not a short list")),
    }
}
