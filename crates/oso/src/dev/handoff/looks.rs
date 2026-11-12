//! What a state holds of how the editor looks (ADR 0047): the visual aids and the theme's
//! name, written and read back.
//!
//! Must not depend on: `app`, `server` or the editor's state beyond these values.

use oso_core::editor::VisualAids;
use serde_json::{Map, Value, json};

/// The visual aids as a state holds them.
pub(crate) fn aids_state(aids: VisualAids) -> Value {
    json!({
        "whitespace": aids.whitespace,
        "current_line": aids.current_line,
        "flash": aids.flash,
        "line_numbers": aids.line_numbers,
    })
}

/// The visual aids, if the state has them. A state from before them has none, and the
/// new instance keeps ADR 0026's defaults.
pub(super) fn parse_aids(state: &Map<String, Value>) -> Result<Option<VisualAids>, String> {
    let Some(aids) = state.get("visual_aids").filter(|aids| !aids.is_null()) else {
        return Ok(None);
    };
    let aids = aids
        .as_object()
        .ok_or("the visual aids are not an object")?;
    let on = |name: &str| {
        aids.get(name)
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("the visual aids have no {name}"))
    };
    Ok(Some(VisualAids {
        whitespace: on("whitespace")?,
        current_line: on("current_line")?,
        flash: on("flash")?,
        line_numbers: on("line_numbers")?,
    }))
}

/// The theme's name, if the state has one, as the built-in themes are named: up to 32
/// lower-case letters, digits and `-`. A state from before themes has none.
pub(super) fn parse_theme(state: &Map<String, Value>) -> Result<Option<String>, String> {
    let named = |name: &&str| {
        (1..=32).contains(&name.len())
            && name
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
    };
    match state.get("theme") {
        None | Some(Value::Null) => Ok(None),
        Some(theme) => theme
            .as_str()
            .filter(named)
            .map(|name| Some(name.to_owned()))
            .ok_or_else(|| "the theme is not a theme's name".into()),
    }
}
