//! The user configuration file, `config.toml`, from its text to the user's keymap and settings
//! (ADR 0108, ADR 0110). Only what the file says: nothing in it can run anything, and it names
//! only registered commands, built-in themes and values of the types below.
//!
//! Must not depend on: `app`, `dispatch` or winit; the dispatcher takes the keymap it builds.
//!
//! [`parse`] and the files under it do no I/O: text in, a finished value out, so they run on any
//! thread and their tests need no files. `watch` is the thread that reads the file, at start and
//! when it changes, and parses it. `outcome` and `in_force` say what that comes to. `trust` reads
//! and writes the record of trusted projects beside it, as `watch` reads the file (ADR 0114).
//! `agent_rules` reads the local agent rules file beside it on the same thread (ADR 0121).
//! `checked` reads each of them, and a file the configuration names (migemo's dictionary,
//! ADR 0116), with ADR 0108's checks.
//!
//! - A syntax error rejects the whole file: no keys and no settings, and every syntax error is
//!   a problem.
//! - Otherwise each entry that is wrong is skipped with a problem, and the rest applies.
//!   Unknown tables and names are skipped too, so a file written for a newer oso still loads.

mod agent_rules;
mod checked;
mod defaults;
mod in_force;
mod indentation;
mod keys;
mod menus;
mod outcome;
mod problems;
mod settings;
mod source;
mod trust;
mod watch;

use std::fmt;

use oso_core::command::Registry;
use oso_core::keymap::Keymap;
use oso_core::menu::Menus;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

pub use agent_rules::RulesReport;
pub use checked::read_checked;
pub use in_force::{InForce, Look};
pub use indentation::MODES;
pub use outcome::{group_writes_note, outcome};
pub use problems::Problem;
pub use settings::Settings;
#[cfg(test)]
pub use source::FILE_NAME;
pub use source::Source;
pub use trust::{Record, TRUST_FILE_NAME, TrustFile};
pub use watch::{Reader, Report};

use problems::{Found, MAX_QUOTED, quoted};

/// What a configuration file holds, and what is wrong in it.
#[derive(Debug)]
pub struct Loaded {
    /// The user's keymap, empty if the file binds no keys. `None` if the file was rejected, and
    /// then nothing in it applies: what was in force stays (ADR 0108).
    pub keys: Option<Keymap>,
    /// The menus in force: the built-in ones with what the file adds, replaces and removes
    /// (ADR 0117). `None` if the file was rejected.
    pub menus: Option<Menus>,
    /// The settings the file gives. All unset if it was rejected.
    pub settings: Settings,
    /// What is wrong, in the order of the file: the first [`MAX_PROBLEMS`](problems::MAX_PROBLEMS)
    /// problems.
    pub problems: Vec<Problem>,
    /// How many problems there were after those.
    pub more: usize,
}

impl Loaded {
    /// Whether a syntax error rejected the whole file.
    #[cfg(test)]
    pub fn rejected(&self) -> bool {
        self.keys.is_none()
    }
}

/// The built-in parts of oso a file is read against.
#[derive(Clone, Copy)]
pub struct Builtin<'b> {
    /// The commands it may name.
    pub registry: &'b Registry,
    /// The default keys its keys are checked against.
    pub defaults: &'b Keymap,
    /// The built-in menus it may replace or remove.
    pub menus: &'b Menus,
    /// The built-in themes' names.
    pub themes: &'b [&'b str],
}

/// Parses `text`, the file's contents, and builds the user's keymap and the menus over what is
/// built in. Never panics, whatever the text.
pub fn parse(text: &str, builtin: Builtin<'_>) -> Loaded {
    let mut found = Found::default();
    let (keys, menus, settings) = match DeTable::parse(text) {
        Ok(root) => {
            let (keys, menus, settings) = apply(root.get_ref(), builtin, &mut found);
            tear_down(root.into_inner());
            (Some(keys), Some(menus), settings)
        }
        Err(first) => {
            syntax_errors(text, &first, &mut found);
            (None, None, Settings::default())
        }
    };
    let (problems, more) = found.placed(text);
    Loaded {
        keys,
        menus,
        settings,
        problems,
        more,
    }
}

/// The stack a thread that calls [`parse`] needs. The parser bounds a key at 80 parts and the
/// nesting of inline tables and arrays at 80, but a dotted key in each nested inline table
/// multiplies the two: a file of a few KiB nests some 6,500 tables, and dropping nested tables
/// recurses. [`parse`] drops what it holds a level at a time; what the parser drops itself, when
/// it rejects a file of any size, took up to 2 MiB in debug and release builds. This is four
/// times that.
pub const STACK: usize = 8 * 1024 * 1024;

/// The largest file whose every syntax error is reported. The parser keeps each one it finds,
/// a few hundred bytes apiece, and a crafted file can hold one every other byte: a larger file
/// reports its first only (ADR 0110).
const RECOVER_UP_TO: usize = 64 * 1024;

/// The syntax errors of `text`, which does not parse: every one, or for a file over
/// [`RECOVER_UP_TO`], the `first`.
fn syntax_errors(text: &str, first: &toml::de::Error, found: &mut Found) {
    // `message`, not `Display`, which finds the error's line by reading the file from the start.
    let at = |error: &toml::de::Error| error.span().map_or(0, |span| span.start);
    if text.len() > RECOVER_UP_TO {
        found.syntax(at(first), first.message());
        found.at(
            at(first),
            format_args!(
                "a file over {} KiB reports only its first syntax error",
                RECOVER_UP_TO / 1024
            ),
        );
        return;
    }
    let (root, errors) = DeTable::parse_recoverable(text);
    for error in &errors {
        found.syntax(at(error), error.message());
    }
    tear_down(root.into_inner());
}

/// The user's keymap, the menus and the settings from the tables of a file that parsed. The
/// menus come first, so that a key can name one whatever the order of the tables.
fn apply(root: &DeTable<'_>, builtin: Builtin<'_>, found: &mut Found) -> (Keymap, Menus, Settings) {
    let Builtin {
        registry,
        defaults,
        themes,
        ..
    } = builtin;
    let mut user = Keymap::new();
    let mut settings = Settings::default();
    let menus = root
        .iter()
        .find(|(name, _)| name.get_ref() == "menus")
        .map_or_else(
            || builtin.menus.clone(),
            |(_, value)| menus::menus(value, registry, builtin.menus, found),
        );
    for (name, value) in root {
        match name.get_ref().as_ref() {
            "menus" => {}
            "keys" => user = keys::keys(value, registry, &menus, defaults, found),
            "theme" => settings.theme = settings::theme(value, themes, found),
            "aids" => settings.aids = settings::aids(value, found),
            "agent-channel" => settings.agent_channel_start = settings::agent_channel(value, found),
            "save" => settings.save = settings::save(value, found),
            "search" => settings.search = settings::search(value, found),
            "jump" => settings.jump = settings::jump(value, found),
            "indent" => settings.indent = indentation::indent(value, found),
            "modes" => settings.modes = indentation::modes(value, found),
            other => found.at(
                name.span().start,
                format_args!("unknown table [{}], skipped", TableName(other)),
            ),
        }
    }
    (user, menus, settings)
}

/// Drops parsed tables a level at a time, so that deep nesting needs no deep stack.
fn tear_down(root: DeTable<'_>) {
    let mut values = vec![DeValue::Table(root)];
    while let Some(mut value) = values.pop() {
        let inner: Box<dyn Iterator<Item = &mut Spanned<DeValue<'_>>>> = match &mut value {
            DeValue::Table(table) => Box::new(table.iter_mut().map(|(_, inner)| inner)),
            DeValue::Array(array) => Box::new(array.iter_mut()),
            _ => continue,
        };
        values
            .extend(inner.map(|inner| std::mem::replace(inner.get_mut(), DeValue::Boolean(false))));
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
    found.at(value.span().start, format_args!("{name} must be a table"));
    None
}

/// A name unknown in `table`, which is skipped.
fn unknown(found: &mut Found, at: usize, name: &str, table: &str) {
    found.at(
        at,
        format_args!("unknown name {} in {table}, skipped", quoted(name)),
    );
}

/// What a TOML value is, for a message.
fn type_name(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) | DeValue::Float(_) => "a number",
        DeValue::Boolean(_) => "true or false",
        DeValue::Datetime(_) => "a date",
        DeValue::Array(_) => "a list",
        DeValue::Table(_) => "a table",
    }
}

/// A table's name as TOML would write it: bare if it can be, quoted otherwise.
struct TableName<'n>(&'n str);

impl fmt::Display for TableName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.0;
        let bare = !name.is_empty()
            && name.len() <= MAX_QUOTED
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if bare {
            f.write_str(name)
        } else {
            quoted(name).fmt(f)
        }
    }
}

#[cfg(test)]
mod jump_tests;
#[cfg(test)]
mod keys_tests;
#[cfg(test)]
mod menus_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod tests;
