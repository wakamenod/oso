//! A `[[message]]` table (ADR 0121, step 5): a rule that finds the line a message from another
//! session makes, and its sender in the capture `from`. Kept apart from the state rules, as it
//! never decides a state: a wrong one is skipped and said.
//!
//! Must not depend on: `app`, `commands`, `dispatch` or `config`; text in, a pattern out.

use std::fmt;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::{MAX_PATTERN_BYTES, Quoted, REGIONS, Read, Reader, SKIPPED, string};
use crate::sessions::messages::{FROM, MessageRule};
use crate::sessions::screen::Region;

/// The most `[[message]]` tables a file holds.
pub const MAX_MESSAGES: usize = 16;

/// The keys a `[[message]]` table takes.
const KEYS: &[&str] = &["name", "region", "pattern", "disable"];

/// The region a message rule reads when it names none.
const DEFAULT_REGION: Region = Region::Bottom(40);

impl Reader<'_> {
    /// The `[[message]]` tables, each kept unless something in it is wrong.
    pub(super) fn messages(&mut self, value: &Spanned<DeValue<'_>>) {
        let DeValue::Array(array) = value.get_ref() else {
            self.problem(
                value.span().start,
                "message must be a list of [[message]] tables",
            );
            return;
        };
        for (n, entry) in array.iter().enumerate() {
            if n == MAX_MESSAGES {
                let why =
                    format_args!("more than {MAX_MESSAGES} message rules; the rest are skipped");
                self.problem(entry.span().start, why);
                return;
            }
            let at = entry.span().start;
            let Some(read) = self.message(entry) else {
                continue;
            };
            let name = match &read {
                Read::Message(message) => &message.name,
                Read::Disable(name) => name,
                Read::Rule(_) => continue,
            };
            let taken = self.parsed.rules.iter().any(|rule| &rule.name == name)
                || self.parsed.messages.iter().any(|other| &other.name == name)
                || self.parsed.disabled.iter().any(|other| other == name);
            if taken {
                self.problem(at, format_args!("rule {} again, skipped", Quoted(name)));
                continue;
            }
            match read {
                Read::Message(message) => self.parsed.messages.push(message),
                Read::Disable(name) => self.parsed.disabled.push(name),
                Read::Rule(_) => {}
            }
        }
    }

    /// One `[[message]]`, or `None`, said, if anything in it is wrong.
    fn message(&mut self, entry: &Spanned<DeValue<'_>>) -> Option<Read> {
        let at = entry.span().start;
        let DeValue::Table(table) = entry.get_ref() else {
            self.problem(at, "each message rule must be a table");
            return None;
        };
        let get = |name: &str| {
            table
                .iter()
                .find(|(key, _)| key.get_ref() == name)
                .map(|(_, value)| value)
        };
        let name = self.name(get("name"), at)?;
        if let Some(disable) = get("disable") {
            return self.disable(table, disable, name);
        }
        let mut skip = |at: usize, why: fmt::Arguments<'_>| {
            self.problem(at, format_args!("rule {}: {why}, {SKIPPED}", Quoted(&name)));
            None
        };
        if let Some(key) = unknown(table) {
            return skip(key.1, format_args!("unknown name {}", Quoted(key.0)));
        }
        let region = match get("region") {
            None => DEFAULT_REGION,
            Some(value) => match string(value.get_ref()).and_then(Region::parse) {
                Some(region) => region,
                None => return skip(value.span().start, format_args!("region must be {REGIONS}")),
            },
        };
        let Some(pattern) = get("pattern") else {
            return skip(at, format_args!("it has no pattern"));
        };
        let at = pattern.span().start;
        let text = string(pattern.get_ref())
            .filter(|text| !text.is_empty() && text.len() <= MAX_PATTERN_BYTES);
        let Some(text) = text else {
            let why = format_args!("pattern must be 1 to {MAX_PATTERN_BYTES} bytes");
            return skip(at, why);
        };
        let compiled = self.compile(text, at, &name, SKIPPED)?;
        if !compiled
            .capture_names()
            .any(|capture| capture == Some(FROM))
        {
            let why = format_args!("its pattern has no capture (?P<{FROM}>…) for the sender");
            self.problem(at, format_args!("rule {}: {why}, {SKIPPED}", Quoted(&name)));
            return None;
        }
        Some(Read::Message(MessageRule {
            name,
            region,
            pattern: compiled,
        }))
    }
}

/// The first key of `table` a message rule does not take, and where it is.
fn unknown<'t>(table: &'t DeTable<'_>) -> Option<(&'t str, usize)> {
    table
        .iter()
        .find(|(key, _)| !KEYS.contains(&key.get_ref().as_ref()))
        .map(|(key, _)| (key.get_ref().as_ref(), key.span().start))
}
