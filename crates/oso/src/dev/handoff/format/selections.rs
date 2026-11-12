//! A window's selections in the state's JSON (ADR 0047, ADR 0090): its cursors, marks and
//! goals, whether the region shows, and its mark ring, written and read back checked.
//!
//! Must not depend on: `app`, `server` or the editor; these are pure functions of values.

use oso_core::selection::{Goal, MARK_RING_SIZE, MarkActive, MarkRing, Selection, SelectionSet};
use serde_json::{Map, Value, json};

use super::{MAX_SELECTIONS, array, size, sizes};
use crate::dev::state::goal_state;

/// A window's selections, marks and whether the region shows, and its mark ring.
pub fn selections_state(selections: &SelectionSet) -> Value {
    json!({
        "primary": selections.primary_index(),
        "selections": selections.iter().map(|selection| json!({
            "cursor": selection.cursor,
            "mark": selection.mark,
            "goal": goal_state(selection.goal),
        })).collect::<Vec<_>>(),
        "mark_active": match selections.mark_active() {
            MarkActive::Inactive => "inactive",
            MarkActive::Active => "active",
            MarkActive::Shifted => "shifted",
        },
        "mark_ring": selections.mark_ring().iter().collect::<Vec<_>>(),
    })
}

pub(super) fn parse_selections(window: &Map<String, Value>) -> Result<SelectionSet, String> {
    let selections = array(window, "selections")?;
    if selections.len() > MAX_SELECTIONS {
        return Err(format!(
            "a window has more than {MAX_SELECTIONS} selections"
        ));
    }
    let selections = selections
        .iter()
        .map(|selection| {
            let selection = selection
                .as_object()
                .ok_or("a selection is not an object")?;
            let goal = match selection.get("goal") {
                None | Some(Value::Null) => None,
                Some(Value::String(goal)) if goal == "line-end" => Some(Goal::LineEnd),
                Some(Value::Object(goal)) => Some(Goal::Column(size(goal, "column")?)),
                Some(_) => return Err("a selection's goal is not understood".to_owned()),
            };
            let mark = match selection.get("mark") {
                None | Some(Value::Null) => None,
                Some(_) => Some(size(selection, "mark")?),
            };
            Ok(Selection {
                cursor: size(selection, "cursor")?,
                mark,
                goal,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut set = SelectionSet::from_parts(selections, size(window, "primary")?)
        .ok_or_else(|| "a window's primary selection does not exist".to_owned())?;
    let active = match window.get("mark_active").and_then(Value::as_str) {
        None | Some("inactive") => MarkActive::Inactive,
        Some("active") => MarkActive::Active,
        Some("shifted") => MarkActive::Shifted,
        Some(_) => return Err("a window's mark_active is not understood".into()),
    };
    set.set_mark_active(active);
    let ring = match window.get("mark_ring") {
        None | Some(Value::Null) => Vec::new(),
        Some(_) => sizes(window, "mark_ring")?,
    };
    if ring.len() > MARK_RING_SIZE {
        return Err(format!("a mark ring holds at most {MARK_RING_SIZE} marks"));
    }
    Ok(set.with_ring(MarkRing::from_newest(ring)))
}
