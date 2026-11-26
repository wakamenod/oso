//! The review layout's own state as a saved review's view keeps it (ADR 0184): the slot's
//! shares, the side selected and what the slot showed, a kind not known read as nothing.
//!
//! Must not depend on: `app`, threads, the editor or the file system.

use std::path::PathBuf;

use serde_json::{Value, json};

use oso_core::review::Side;

use super::{bytes_value, side_name, side_of, value_bytes};

/// What the review layout kept of itself as its review was left (ADR 0184): the slot's share
/// of its row and of the column it shares with the window below it, in thousandths, which side
/// was selected, and what the slot showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavedLayout {
    pub(crate) slot_selected: bool,
    pub(crate) slot_permille: u16,
    pub(crate) ask_permille: Option<u16>,
    pub(crate) slot: SlotShows,
}

/// What the review layout's slot showed: never a buffer that may hold secrets or is withheld
/// from agents, which is kept as nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlotShows {
    Nothing,
    /// The review's message of comments to send, made again from the comments not sent.
    Message,
    /// A file, at `line` from 1, `column` bytes into it, `above` rows below the window's top.
    Source {
        path: PathBuf,
        line: u32,
        column: u32,
        above: u32,
    },
    /// A side of a file of the review, read-only, at `line` of that side from 1.
    Side {
        path: Box<[u8]>,
        side: Side,
        line: u32,
        above: u32,
    },
}

pub(super) fn layout_value(layout: &SavedLayout) -> Value {
    let shows = match &layout.slot {
        SlotShows::Nothing => json!({ "kind": "nothing" }),
        SlotShows::Message => json!({ "kind": "message" }),
        SlotShows::Source {
            path,
            line,
            column,
            above,
        } => match path.to_str() {
            // A name that is not UTF-8 is kept as nothing, as a root is never one.
            Some(path) => json!({
                "kind": "source", "path": path, "line": line, "column": column, "above": above
            }),
            None => json!({ "kind": "nothing" }),
        },
        SlotShows::Side {
            path,
            side,
            line,
            above,
        } => json!({
            "kind": "side", "path": bytes_value(path), "side": side_name(*side),
            "line": line, "above": above
        }),
    };
    let mut value = json!({
        "slot_selected": layout.slot_selected,
        "slot": layout.slot_permille,
        "shows": shows,
    });
    if let (Some(ask), Some(map)) = (layout.ask_permille, value.as_object_mut()) {
        map.insert("ask".to_owned(), json!(ask));
    }
    value
}

/// The layout [`layout_value`] wrote; a share past the whole is cut to it, and a slot of a
/// kind this does not know shows nothing.
pub(super) fn layout_of(value: &Value) -> Option<SavedLayout> {
    let share = |value: &Value| u16::try_from(value.as_u64()?.min(1000)).ok();
    let shows = value.get("shows")?;
    let number = |name: &str| u32::try_from(shows.get(name)?.as_u64()?).ok();
    let slot = match shows.get("kind").and_then(Value::as_str) {
        Some("message") => Some(SlotShows::Message),
        Some("source") => (|| {
            Some(SlotShows::Source {
                path: PathBuf::from(shows.get("path")?.as_str()?),
                line: number("line")?,
                column: number("column")?,
                above: number("above")?,
            })
        })(),
        Some("side") => (|| {
            Some(SlotShows::Side {
                path: value_bytes(shows.get("path")?)?,
                side: side_of(shows.get("side")?)?,
                line: number("line")?,
                above: number("above")?,
            })
        })(),
        _ => None,
    };
    Some(SavedLayout {
        slot_selected: value.get("slot_selected")?.as_bool()?,
        slot_permille: share(value.get("slot")?)?,
        ask_permille: value.get("ask").and_then(share),
        slot: slot.unwrap_or(SlotShows::Nothing),
    })
}
