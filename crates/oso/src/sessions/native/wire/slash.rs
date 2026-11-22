//! The slash commands a native session's CLI offers (ADR 0165, step 2d): `initialize`'s answer
//! lists each with its description, argument hint and aliases; `system/init` lists their names.
//!
//! Must not depend on: `rows`, threads or the UI; plain functions over a parsed line.
//!
//! - Their text is the CLI's and the project's own (a skill's description is the project's
//!   file), so it is untrusted: at most [`MAX_COMMANDS`] and [`MAX_ALIASES`] each, each hint and
//!   description cut to its first line and [`MAX_NAME`]. A name or alias that is empty, longer
//!   than [`MAX_NAME`], or holds a space, a control, an invisible or a bidi character is no
//!   command's: it could not be typed after a `/`, or could pass for another's.

use serde_json::{Map, Value};

use super::{MAX_NAME, name};

/// The most commands kept of one list.
pub const MAX_COMMANDS: usize = 512;
/// The most aliases kept of one command.
pub const MAX_ALIASES: usize = 8;

/// A slash command the CLI offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// What follows the `/`.
    pub name: String,
    /// Its description's first line.
    pub description: String,
    /// What it takes after its name, such as `<model>`.
    pub hint: String,
    /// Other names it answers to, such as a plugin's `plugin:name`.
    pub aliases: Vec<String>,
    /// One of the CLI's own, not a skill's or a plugin's.
    pub builtin: bool,
}

/// The commands `initialize`'s answer lists in `response`, if it lists any.
pub(super) fn commands(response: &Map<String, Value>) -> Option<Vec<SlashCommand>> {
    let listed = response.get("commands")?.as_array()?;
    let commands = listed
        .iter()
        .filter_map(Value::as_object)
        .filter_map(command)
        .take(MAX_COMMANDS)
        .collect();
    Some(commands)
}

fn command(object: &Map<String, Value>) -> Option<SlashCommand> {
    let called = typable(object.get("name")?.as_str()?)?;
    let text = |key| object.get(key).and_then(Value::as_str).unwrap_or("");
    let line = |key| text(key).lines().next().unwrap_or("").trim();
    let aliases = object
        .get("aliases")
        .and_then(Value::as_array)
        .map(|aliases| {
            aliases
                .iter()
                .filter_map(Value::as_str)
                .filter_map(typable)
                .take(MAX_ALIASES)
                .collect()
        })
        .unwrap_or_default();
    Some(SlashCommand {
        name: called,
        description: name(line("description")),
        hint: name(line("argumentHint")),
        aliases,
        builtin: object.get("builtin").and_then(Value::as_bool) == Some(true),
    })
}

/// The names `system/init` lists under `key`.
pub(super) fn names(object: &Map<String, Value>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .filter_map(typable)
                .take(MAX_COMMANDS)
                .collect()
        })
        .unwrap_or_default()
}

/// `text` as a command's name, if it can be one.
fn typable(text: &str) -> Option<String> {
    let text = text.strip_prefix('/').unwrap_or(text);
    let plain = !text.is_empty() && text.len() <= MAX_NAME && !text.chars().any(unplain);
    plain.then(|| text.to_owned())
}

/// Whether `c` has no place in a name: a space, a control, or a character that shows as
/// nothing or turns the text around.
fn unplain(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(
            c,
            '\u{ad}'
                | '\u{34f}'
                | '\u{61c}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
        )
}
