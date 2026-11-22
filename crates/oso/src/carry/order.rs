//! The order of the buffers a state hands over, the most recently used first, as Emacs's desktop
//! saves `buffer-list`'s (ADR 0171): the ids of the state's buffers, written and read back
//! checked. A state from before it says nothing of it, and the buffers keep the order they
//! came in.
//!
//! Must not depend on: `app` or the host's window; these are pure functions of the
//! editor state and values.

use std::collections::{HashMap, HashSet};

use oso_core::editor::Editor;
use serde_json::{Map, Value};

/// The ids of the state's `buffers`, the most recently used first, as the editor has them.
/// A buffer the state does not hand over is left out.
pub(super) fn state(editor: &Editor, buffers: &[Value]) -> Value {
    let carried: HashSet<u64> = buffers
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_u64))
        .collect();
    (editor.buffers_by_recency().into_iter())
        .map(oso_core::buffer::BufferId::get)
        .filter(|id| carried.contains(id))
        .map(Value::from)
        .collect()
}

/// Which of the state's `buffers` its order names, by their places in `buffers`, the most
/// recently used first, once each. An id no buffer has is passed over, as an order only sorts.
///
/// # Errors
///
/// For an order that is not a list of ids, or longer than the state's buffers.
pub(super) fn parse(state: &Map<String, Value>, buffers: &[Value]) -> Result<Vec<usize>, String> {
    let order = match state.get("order") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        // The buffers are at most `MAX_BUFFERS`, checked before this.
        Some(Value::Array(order)) if order.len() <= buffers.len() => order,
        Some(_) => {
            return Err(
                "the order is not a list of at most as many ids as the state has buffers".into(),
            );
        }
    };
    let at: HashMap<u64, usize> = (buffers.iter().enumerate())
        .filter_map(|(at, entry)| Some((entry.get("id")?.as_u64()?, at)))
        .collect();
    let mut seen = HashSet::new();
    let mut places = Vec::new();
    for id in order {
        let id = id.as_u64().ok_or("an id in the order is not a number")?;
        if let Some(&place) = at.get(&id)
            && seen.insert(place)
        {
            places.push(place);
        }
    }
    Ok(places)
}
