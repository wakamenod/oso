//! What a native session's status line reads of the control protocol's answers (ADR 0165,
//! part 3a): the models `initialize` lists, and the usage `get_usage` gives.
//!
//! Must not depend on: `rows`, threads or the UI; plain functions over a parsed line.
//!
//! - The models' text is the CLI's, but shown in a modeline, so it is held to what reads as
//!   itself ([`readable`]): at most [`MAX_MODELS`], and a model whose id or name is empty,
//!   longer than [`MAX_NAME`], or holds what its kind does not (ASCII alone), is left out.
//! - Of `get_usage`'s answer only the two named windows, `five_hour` and `seven_day`, and the
//!   session's cost are read: its other windows go by names of the CLI's own, which say nothing
//!   oso can show (measured, CLI 2.1.330).

use serde_json::{Map, Value};

use super::MAX_NAME;

/// The most models kept of one list.
pub const MAX_MODELS: usize = 64;

/// A model `initialize` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// What `--model` takes for it: an alias, such as `opus`, or an id.
    pub value: String,
    /// The id it stands for, such as `claude-opus-5-5`.
    pub resolved: String,
    /// What it is called, such as `Opus 5.5`.
    pub display: String,
}

/// What `get_usage` says.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    /// How much of each named window is used, in percent, where it says.
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    /// What the session cost so far, in US dollars, a resumed one's earlier processes included
    /// (measured).
    pub cost_usd: Option<f64>,
}

/// The models `initialize`'s answer lists in `response`, if it lists any.
pub(super) fn models(response: &Map<String, Value>) -> Option<Vec<Model>> {
    let listed = response.get("models")?.as_array()?;
    let models = listed
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|model| {
            let text =
                |key, kind| (model.get(key).and_then(Value::as_str)).filter(|t| readable(t, kind));
            let (value, resolved) = (text("value", Kind::Id)?, text("resolvedModel", Kind::Id)?);
            let display = text("displayName", Kind::Label)?;
            Some(Model {
                value: value.to_owned(),
                resolved: resolved.to_owned(),
                display: display.to_owned(),
            })
        })
        .take(MAX_MODELS)
        .collect();
    Some(models)
}

/// What a part of the status line is, which says what it may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A model's id or alias: ASCII letters, digits and `._:/@[]-`.
    Id,
    /// A permission mode or an effort level: ASCII letters, digits, `-` and `_`.
    Word,
    /// A model's name: printable ASCII, its words one space apart.
    Label,
}

/// Whether `text` can be shown as itself in the status line as a part of kind `kind`: not
/// empty, at most [`MAX_NAME`] bytes, and only what that kind holds. ASCII alone, so that no
/// control, invisible, bidi or look-alike character, nor a dot like the ` · ` the line's parts
/// are joined with, gets in: a part could then pass for another, a model for a mode.
pub fn readable(text: &str, kind: Kind) -> bool {
    let fits = match kind {
        Kind::Id => text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/@[]-".contains(&b)),
        Kind::Word => text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        Kind::Label => {
            text.bytes().all(|b| (b' '..=b'~').contains(&b))
                && !text.starts_with(' ')
                && !text.ends_with(' ')
                && !text.contains("  ")
        }
    };
    !text.is_empty() && text.len() <= MAX_NAME && fits
}

/// What `get_usage`'s answer says in `response`, if it is one: it holds `rate_limits` or
/// `session`.
pub(super) fn usage(response: &Map<String, Value>) -> Option<Usage> {
    let limits = response.get("rate_limits").and_then(Value::as_object);
    let session = response.get("session").and_then(Value::as_object);
    if limits.is_none() && session.is_none() {
        return None;
    }
    let window = |key| {
        let window = limits?.get(key)?.as_object()?;
        window.get("utilization").and_then(Value::as_f64)
    };
    Some(Usage {
        five_hour: window("five_hour"),
        seven_day: window("seven_day"),
        cost_usd: session
            .and_then(|session| session.get("total_cost_usd"))
            .and_then(Value::as_f64),
    })
}
