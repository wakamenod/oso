//! `[lsp]`: when language servers start, and oso's built-in table of servers with the
//! person's changes to it (ADR 0138, Q2 and Q3). The table needs no configuration:
//! `[lsp.servers.<name>]` changes only the keys it names, turns an entry off, or adds one.
//! A mode has at most one server: an entry of the person's wins over a built-in one, and of
//! two of the person's, the first in the file wins and the second is reported. `options` is
//! turned from TOML into JSON here, on the configuration's thread.
//!
//! Must not depend on: `app`, `dispatch` or the file system; nothing is looked for on disk
//! here. The program is found when its server starts, on the server's thread.

use oso_core::buffer::Mode;
use serde_json::{Map, Number, Value, json};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::indentation::MODES;
use super::problems::{Found, cut, quoted};
use super::settings::{env_names, switch};
use super::{table, type_name, unknown};

/// The most entries `[lsp.servers]` takes.
pub(super) const MAX_SERVERS: usize = 64;
/// The longest entry name, in bytes.
pub(super) const MAX_NAME: usize = 64;
/// The most words in a `command`, and the longest word, in bytes.
pub(super) const MAX_WORDS: usize = 64;
pub(super) const MAX_WORD: usize = 4096;
/// How deep `options` may nest.
pub(super) const MAX_DEPTH: usize = 32;

/// When a server starts (ADR 0138, Q4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StartWhen {
    /// At the person's first visit of a buffer in a trusted project.
    #[default]
    Trusted,
    /// Only on `lsp-start`.
    Manual,
}

/// One server of the table, with the person's changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub modes: Vec<Mode>,
    /// The program, a name on `PATH` or an absolute path, and its arguments.
    pub command: Vec<String>,
    /// The program is also looked for in Cargo's `bin` directory, after `PATH`.
    pub in_cargo_bin: bool,
    /// More names from oso's environment.
    pub env: Vec<String>,
    /// The section of the settings it asks `workspace/configuration` for.
    pub section: Option<String>,
    pub options: Option<Value>,
}

/// `[lsp]`, with the table it leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lsp {
    pub start: StartWhen,
    pub servers: Vec<Entry>,
}

impl Default for Lsp {
    fn default() -> Self {
        Self {
            start: StartWhen::default(),
            servers: builtin(),
        }
    }
}

impl Lsp {
    /// The server of `mode`, if one is on.
    pub fn for_mode(&self, mode: Mode) -> Option<&Entry> {
        self.servers
            .iter()
            .find(|entry| entry.modes.contains(&mode))
    }
}

/// oso's own table (ADR 0138, Q2). Step 3 has rust-analyzer; step 5 brings the rest.
fn builtin() -> Vec<Entry> {
    vec![Entry {
        name: "rust-analyzer".into(),
        modes: vec![Mode::Rust],
        command: vec!["rust-analyzer".into()],
        in_cargo_bin: true,
        // rustup's proxy needs them when they are not the defaults (ADR 0138, step 3's Q3).
        env: vec!["CARGO_HOME".into(), "RUSTUP_HOME".into()],
        section: Some("rust-analyzer".into()),
        // Its own target directory, so that its checks never wait on `cargo`'s lock.
        options: Some(json!({"cargo": {"targetDir": true}})),
    }]
}

/// The changes one `[lsp.servers.<name>]` names.
#[derive(Debug, Default)]
struct Change {
    name: String,
    at: usize,
    enabled: Option<bool>,
    modes: Option<Vec<Mode>>,
    command: Option<Vec<String>>,
    env: Option<Vec<String>>,
    options: Option<Value>,
}

/// `[lsp]`.
pub(super) fn lsp(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Lsp {
    let mut lsp = Lsp::default();
    let Some(entries) = table(value, "[lsp]", found) else {
        return lsp;
    };
    let mut changes = Vec::new();
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "start" => {
                if let Some(start) = start(value, found) {
                    lsp.start = start;
                }
            }
            "servers" => changes = servers(value, found),
            other => unknown(found, name.span().start, other, "[lsp]"),
        }
    }
    lsp.servers = apply(builtin(), changes, found);
    lsp
}

fn start(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<StartWhen> {
    match value.get_ref() {
        DeValue::String(text) if text == "trusted" => Some(StartWhen::Trusted),
        DeValue::String(text) if text == "manual" => Some(StartWhen::Manual),
        other => {
            let what = match other {
                DeValue::String(text) => quoted(cut(text.as_ref())).to_string(),
                other => type_name(other).to_owned(),
            };
            found.at(
                value.span().start,
                format_args!("[lsp] start takes \"trusted\" or \"manual\", not {what}"),
            );
            None
        }
    }
}

/// `[lsp.servers]`'s tables, in the file's order.
fn servers(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Vec<Change> {
    let Some(entries) = table(value, "[lsp.servers]", found) else {
        return Vec::new();
    };
    if entries.len() > MAX_SERVERS {
        found.at(
            value.span().start,
            format_args!("[lsp.servers] takes at most {MAX_SERVERS} servers"),
        );
        return Vec::new();
    }
    let mut changes = Vec::new();
    for (name, value) in entries {
        let key = name.get_ref().as_ref();
        let plain = !key.is_empty()
            && key.len() <= MAX_NAME
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if !plain {
            found.at(
                name.span().start,
                format_args!(
                    "a server's name takes letters, digits, -, _ and ., at most {MAX_NAME}, not {}",
                    quoted(cut(key))
                ),
            );
            continue;
        }
        if let Some(change) = server(key, name.span().start, value, found) {
            changes.push(change);
        }
    }
    changes
}

/// One `[lsp.servers.<name>]`.
fn server(
    name: &str,
    at: usize,
    value: &Spanned<DeValue<'_>>,
    found: &mut Found,
) -> Option<Change> {
    let label = format!("[lsp.servers.{name}]");
    let entries = table(value, &label, found)?;
    let mut change = Change {
        name: name.to_owned(),
        at,
        ..Change::default()
    };
    for (key, value) in entries {
        match key.get_ref().as_ref() {
            "enabled" => change.enabled = switch(value, &label, "enabled", found),
            "modes" => change.modes = modes(value, &label, found),
            "command" => change.command = command(value, &label, found),
            "env" => change.env = Some(env_names(value, &label, found)),
            "options" => change.options = options(value, &label, found),
            other => unknown(found, key.span().start, other, &label),
        }
    }
    Some(change)
}

/// `modes`: names of [`MODES`] but `fundamental`, at least one.
fn modes(value: &Spanned<DeValue<'_>>, label: &str, found: &mut Found) -> Option<Vec<Mode>> {
    let DeValue::Array(names) = value.get_ref() else {
        found.at(
            value.span().start,
            format_args!(
                "{label} modes takes a list of modes, such as [\"toml\"], not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    let mut kept = Vec::new();
    for name in names {
        let mode = match name.get_ref() {
            DeValue::String(text) => MODES
                .iter()
                .find(|(known, mode)| *known == text.as_ref() && *mode != Mode::Fundamental)
                .map(|(_, mode)| *mode),
            _ => None,
        };
        match mode {
            Some(mode) if !kept.contains(&mode) => kept.push(mode),
            Some(_) => {}
            None => found.at(
                name.span().start,
                format_args!("{label} modes takes rust, markdown or toml, skipped"),
            ),
        }
    }
    (!kept.is_empty()).then_some(kept)
}

/// `command`: a program, a name on `PATH` or an absolute path, then its arguments.
fn command(value: &Spanned<DeValue<'_>>, label: &str, found: &mut Found) -> Option<Vec<String>> {
    let refuse = |found: &mut Found, what: &str| {
        found.at(
            value.span().start,
            format_args!("{label} command takes a list of words, the program first: {what}"),
        );
        None
    };
    let DeValue::Array(words) = value.get_ref() else {
        return refuse(found, type_name(value.get_ref()));
    };
    if words.is_empty() || words.len() > MAX_WORDS {
        return refuse(found, &format!("from 1 to {MAX_WORDS} words"));
    }
    let mut kept = Vec::with_capacity(words.len());
    for word in words {
        match word.get_ref() {
            DeValue::String(text) if text.len() <= MAX_WORD && !text.contains('\0') => {
                kept.push(text.as_ref().to_owned());
            }
            _ => return refuse(found, &format!("words of at most {MAX_WORD} bytes")),
        }
    }
    let program = std::path::Path::new(&kept[0]);
    let plain = program.file_name() == Some(program.as_os_str());
    if !plain && !program.is_absolute() {
        return refuse(found, "the program is a name on PATH or an absolute path");
    }
    Some(kept)
}

/// `options`: a table, as JSON.
fn options(value: &Spanned<DeValue<'_>>, label: &str, found: &mut Found) -> Option<Value> {
    let DeValue::Table(entries) = value.get_ref() else {
        found.at(
            value.span().start,
            format_args!(
                "{label} options takes a table, not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    match table_json(entries, 0) {
        Ok(json) => Some(json),
        Err(why) => {
            found.at(
                value.span().start,
                format_args!("{label} options {why}, skipped"),
            );
            None
        }
    }
}

fn table_json(entries: &DeTable<'_>, depth: usize) -> Result<Value, String> {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(
            key.get_ref().to_string(),
            json_of(value.get_ref(), depth + 1)?,
        );
    }
    Ok(Value::Object(map))
}

/// `value` as JSON: a date as its text, and no number JSON cannot hold.
fn json_of(value: &DeValue<'_>, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!("nests more than {MAX_DEPTH} deep"));
    }
    Ok(match value {
        DeValue::String(text) => Value::String(text.to_string()),
        DeValue::Integer(n) => i64::from_str_radix(n.as_str(), n.radix())
            .map(Value::from)
            .map_err(|_| format!("holds a number out of range: {}", cut(n)))?,
        DeValue::Float(n) => n
            .as_str()
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| format!("holds a number JSON cannot: {}", cut(n)))?,
        DeValue::Boolean(on) => Value::Bool(*on),
        DeValue::Datetime(at) => Value::String(at.to_string()),
        DeValue::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| json_of(item.get_ref(), depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        DeValue::Table(entries) => table_json(entries, depth)?,
    })
}

/// The table after `changes`, in the file's order: each changes the built-in entry of its
/// name, or adds one. Each mode then keeps the first entry that names it, the person's before
/// the built-in ones; a later one of the person's is reported.
fn apply(mut entries: Vec<Entry>, changes: Vec<Change>, found: &mut Found) -> Vec<Entry> {
    let mut mine: Vec<Entry> = Vec::new();
    let mut off: Vec<String> = Vec::new();
    for change in changes {
        let existing = entries.iter().position(|entry| entry.name == change.name);
        if change.enabled == Some(false) {
            mine.retain(|kept| kept.name != change.name);
            off.push(change.name);
            continue;
        }
        let mut entry = match existing {
            Some(index) => entries[index].clone(),
            None => Entry {
                name: change.name.clone(),
                modes: Vec::new(),
                command: Vec::new(),
                in_cargo_bin: false,
                env: Vec::new(),
                section: Some(change.name.clone()),
                options: None,
            },
        };
        if let Some(modes) = change.modes {
            entry.modes = modes;
        }
        if let Some(command) = change.command {
            entry.command = command;
            // A command of the person's is looked for where it says, on `PATH` or at its path.
            entry.in_cargo_bin = false;
        }
        if let Some(env) = change.env {
            for name in env {
                if !entry.env.contains(&name) {
                    entry.env.push(name);
                }
            }
        }
        if let Some(options) = &change.options {
            let base = entry
                .options
                .get_or_insert_with(|| Value::Object(Map::new()));
            oso_lsp::protocol::merge(base, options);
        }
        if entry.command.is_empty() || entry.modes.is_empty() {
            found.at(
                change.at,
                format_args!(
                    "[lsp.servers.{}] needs a command and modes, skipped",
                    change.name
                ),
            );
            continue;
        }
        if let Some(clash) = mine
            .iter()
            .find(|kept| kept.modes.iter().any(|mode| entry.modes.contains(mode)))
        {
            found.at(
                change.at,
                format_args!(
                    "[lsp.servers.{}] names a mode [lsp.servers.{}] has already, skipped",
                    change.name, clash.name
                ),
            );
            continue;
        }
        if let Some(index) = existing {
            entries.remove(index);
        }
        mine.retain(|kept| kept.name != entry.name);
        mine.push(entry);
    }
    entries.retain(|entry| !off.contains(&entry.name));
    // The person's entries win the modes they name.
    entries.retain(|entry| {
        !mine
            .iter()
            .any(|kept| kept.modes.iter().any(|mode| entry.modes.contains(mode)))
    });
    mine.extend(entries);
    mine
}
