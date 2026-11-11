//! The user configuration file, `config.toml`, from its text to the user's keymap and settings
//! (ADR 0108, ADR 0110). Only what the file says: nothing in it can run anything, and it names
//! only registered commands, built-in themes and values of the types below.
//!
//! Must not depend on: `app`, `dispatch`, the file system or winit; text in, a finished value
//! out, so it runs on any thread and its tests need no files. Reading and watching the file are
//! the caller's.
//!
//! - A syntax error rejects the whole file: no keys and no settings, and every syntax error is
//!   a problem.
//! - Otherwise each entry that is wrong is skipped with a problem, and the rest applies.
//!   Unknown tables and names are skipped too, so a file written for a newer oso still loads.

mod keys;
mod problems;
mod settings;

use oso_core::command::Registry;
use oso_core::keymap::Keymap;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

pub use problems::Problem;
pub use settings::Settings;

use problems::{Found, MAX_QUOTED, quoted};

/// What a configuration file holds, and what is wrong in it.
#[derive(Debug)]
pub struct Loaded {
    /// The user's keymap, empty if the file binds no keys. `None` if the file was rejected, and
    /// then nothing in it applies: what was in force stays (ADR 0108).
    pub keys: Option<Keymap>,
    /// The settings the file gives. All unset if it was rejected.
    pub settings: Settings,
    /// What is wrong, in the order of the file.
    pub problems: Vec<Problem>,
}

impl Loaded {
    /// Whether a syntax error rejected the whole file.
    pub fn rejected(&self) -> bool {
        self.keys.is_none()
    }
}

/// Parses `text`, the file's contents, and builds the user's keymap over the commands in
/// `registry`, checked against the default keys in `defaults`. `themes` are the built-in themes'
/// names. Never panics, whatever the text.
pub fn parse(text: &str, registry: &Registry, defaults: &Keymap, themes: &[&str]) -> Loaded {
    let mut found = Found::default();
    let (root, errors) = DeTable::parse_recoverable(text);
    for error in &errors {
        found.syntax(error.span().map_or(0, |span| span.start), error.message());
    }
    if !found.is_empty() {
        return Loaded {
            keys: None,
            settings: Settings::default(),
            problems: found.placed(text),
        };
    }
    let mut user = Keymap::new();
    let mut settings = Settings::default();
    for (name, value) in root.get_ref() {
        match name.get_ref().as_ref() {
            "keys" => user = keys::keys(value, registry, defaults, &mut found),
            "theme" => settings.theme = settings::theme(value, themes, &mut found),
            "aids" => settings.aids = settings::aids(value, &mut found),
            "agent-channel" => {
                settings.agent_channel_start = settings::agent_channel(value, &mut found)
            }
            other => found.at(
                name.span().start,
                format!("unknown table [{}], skipped", table_name(other)),
            ),
        }
    }
    Loaded {
        keys: Some(user),
        settings,
        problems: found.placed(text),
    }
}

/// The table `value`, called `name` in a message if it is something else.
fn table<'v, 'i>(
    value: &'v Spanned<DeValue<'i>>,
    name: &str,
    found: &mut Found,
) -> Option<&'v DeTable<'i>> {
    if let DeValue::Table(table) = value.get_ref() {
        return Some(table);
    }
    found.at(value.span().start, format!("{name} must be a table"));
    None
}

/// A name unknown in `table`, which is skipped.
fn unknown(found: &mut Found, at: usize, name: &str, table: &str) {
    found.at(
        at,
        format!("unknown name {} in {table}, skipped", quoted(name)),
    );
}

/// A table's name as TOML would write it: bare if it can be, quoted otherwise.
fn table_name(name: &str) -> String {
    let bare = !name.is_empty()
        && name.len() <= MAX_QUOTED
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if bare { name.to_owned() } else { quoted(name) }
}

#[cfg(test)]
mod keys_tests;
#[cfg(test)]
mod tests;
