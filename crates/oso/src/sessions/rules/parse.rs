//! A rules file's text to its rules (ADR 0121, Q4): every check of its format and its limits.
//!
//! Must not depend on: `app`, `commands`, `dispatch` or `config`; text in, rules out, no I/O,
//! so it runs on any thread and its tests need no files.
//!
//! - A syntax error or a missing or unknown `version` refuses the whole file.
//! - Otherwise a rule with anything wrong in it, an unknown name included, is skipped with a
//!   problem: a misspelt matcher must not leave a rule that matches more than meant.

mod message;
mod recap;

#[cfg(test)]
pub use message::MAX_MESSAGES;

use std::fmt;

use regex::{Regex, RegexBuilder};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::{
    MAX_DEPTH, MAX_MATCHERS, MAX_PATTERN_BYTES, MAX_PATTERNS, MAX_RULES, PATTERN_SIZE_LIMIT,
    Parsed, Rule, Source, Test,
};
use crate::sessions::screen::Region;
use crate::sessions::state::{Reason, Target};

/// The most problems kept; the rest are counted.
pub const MAX_PROBLEMS: usize = 20;

/// The only version of the format.
const VERSION: &str = "1";

/// The largest priority, either way.
const MAX_PRIORITY: i64 = 100_000;

/// The longest rule name, in bytes.
const MAX_NAME_BYTES: usize = 64;

/// The most bytes of a name or value quoted in a problem.
const MAX_QUOTED: usize = 64;

/// The keys of a rule that are not matchers.
const RULE_KEYS: &[&str] = &[
    "name",
    "state",
    "reason",
    "priority",
    "region",
    "visible",
    "disable",
    "recap",
    "recap_region",
];

/// What a problem says is not used: the rule.
const SKIPPED: &str = "skipped";

/// What a region must be, in a problem.
const REGIONS: &str = "one of title, progress, bottom(N) with N from 1 to 40, prompt_box, \
                       after_last_rule or above_prompt_box";

/// Parses `text`, a rules file from `source`. Never panics, whatever the text.
pub fn parse(text: &str, source: Source) -> Parsed {
    let mut reader = Reader {
        text,
        source,
        parsed: Parsed::default(),
        patterns: 0,
    };
    match DeTable::parse(text) {
        Ok(root) => {
            reader.root(root.get_ref());
            tear_down(root.into_inner());
        }
        Err(error) => {
            let at = error.span().map_or(0, |span| span.start);
            reader.problem(at, error.message());
            reader.parsed.refused = true;
        }
    }
    if reader.parsed.refused {
        reader.parsed.rules.clear();
        reader.parsed.messages.clear();
        reader.parsed.disabled.clear();
    }
    reader.parsed
}

/// A rule's table read, or the name it disables.
enum Read {
    Rule(Rule),
    Message(crate::sessions::messages::MessageRule),
    Disable(String),
}

struct Reader<'t> {
    text: &'t str,
    source: Source,
    parsed: Parsed,
    /// Patterns compiled so far.
    patterns: usize,
}

impl Reader<'_> {
    fn problem(&mut self, at: usize, message: impl fmt::Display) {
        if self.parsed.problems.len() >= MAX_PROBLEMS {
            self.parsed.more += 1;
            return;
        }
        let at = at.min(self.text.len());
        let line = self.text.as_bytes()[..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;
        self.parsed.problems.push(format!("{line}: {message}"));
    }

    fn root(&mut self, root: &DeTable<'_>) {
        let version = root.iter().find(|(key, _)| key.get_ref() == "version");
        match version.map(|(_, value)| value) {
            Some(value) if integer(value.get_ref()) == Some(VERSION) => {}
            Some(value) => {
                self.problem(value.span().start, "version must be 1; the file is refused");
                self.parsed.refused = true;
                return;
            }
            None => {
                self.problem(0, "version = 1 is missing; the file is refused");
                self.parsed.refused = true;
                return;
            }
        }
        for (key, value) in root {
            match key.get_ref().as_ref() {
                "version" => {}
                "rule" => self.rules(value),
                "message" => self.messages(value),
                other => self.problem(
                    key.span().start,
                    format_args!("unknown name {}, skipped", Quoted(other)),
                ),
            }
        }
    }

    fn rules(&mut self, value: &Spanned<DeValue<'_>>) {
        let DeValue::Array(array) = value.get_ref() else {
            self.problem(value.span().start, "rule must be a list of [[rule]] tables");
            return;
        };
        for (n, entry) in array.iter().enumerate() {
            if n == MAX_RULES {
                self.problem(
                    entry.span().start,
                    format_args!("more than {MAX_RULES} rules; the rest are skipped"),
                );
                return;
            }
            let at = entry.span().start;
            let Some(read) = self.rule(entry) else {
                continue;
            };
            let name = match &read {
                Read::Rule(rule) => &rule.name,
                Read::Message(message) => &message.name,
                Read::Disable(name) => name,
            };
            let taken = self.parsed.rules.iter().any(|rule| &rule.name == name)
                || self.parsed.messages.iter().any(|other| &other.name == name)
                || self.parsed.disabled.iter().any(|other| other == name);
            if taken {
                let message = format_args!("rule {} again, skipped", Quoted(name));
                self.problem(at, message);
                continue;
            }
            match read {
                Read::Rule(rule) => self.parsed.rules.push(rule),
                Read::Message(message) => self.parsed.messages.push(message),
                Read::Disable(name) => self.parsed.disabled.push(name),
            }
        }
    }

    /// One `[[rule]]`, or `None`, said, if anything in it is wrong.
    fn rule(&mut self, entry: &Spanned<DeValue<'_>>) -> Option<Read> {
        let at = entry.span().start;
        let DeValue::Table(table) = entry.get_ref() else {
            self.problem(at, "each rule must be a table");
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
        let skip = |reader: &mut Self, at: usize, why: fmt::Arguments<'_>| {
            reader.problem(at, format_args!("rule {}: {why}, skipped", Quoted(&name)));
        };
        let Some(state) = get("state") else {
            skip(self, at, format_args!("it has no state"));
            return None;
        };
        let target = self.target(state, get("reason"), get("visible"), &name)?;
        let priority = match get("priority") {
            None => 0,
            Some(value) => {
                let number = integer(value.get_ref()).and_then(|n| n.parse::<i64>().ok());
                match number.filter(|n| n.abs() <= MAX_PRIORITY) {
                    Some(n) => n,
                    None => {
                        let message = format_args!(
                            "priority must be a whole number from -{MAX_PRIORITY} to {MAX_PRIORITY}"
                        );
                        skip(self, value.span().start, message);
                        return None;
                    }
                }
            }
        };
        let region = match get("region").map(|value| (value, string(value.get_ref()))) {
            Some((_, Some(region))) if Region::parse(region).is_some() => Region::parse(region)?,
            Some((value, _)) => {
                skip(
                    self,
                    value.span().start,
                    format_args!("region must be {REGIONS}"),
                );
                return None;
            }
            None => {
                skip(self, at, format_args!("it has no region"));
                return None;
            }
        };
        let mut matchers = 0;
        let test = self.test(table, 0, &mut matchers, &name)?;
        if test.is_empty() {
            skip(self, at, format_args!("it has nothing to match"));
            return None;
        }
        let recap = self.recap(get("recap"), get("recap_region"), region, &name);
        Some(Read::Rule(Rule {
            name,
            target,
            priority,
            region,
            source: self.source,
            recap,
            test,
        }))
    }

    fn name(&mut self, value: Option<&Spanned<DeValue<'_>>>, at: usize) -> Option<String> {
        let Some(value) = value else {
            self.problem(at, "a rule has no name, skipped");
            return None;
        };
        let valid = string(value.get_ref()).filter(|name| {
            !name.is_empty()
                && name.len() <= MAX_NAME_BYTES
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
        if valid.is_none() {
            self.problem(
                value.span().start,
                format_args!("a rule's name must be 1 to {MAX_NAME_BYTES} letters, digits, - or _; the rule is skipped"),
            );
        }
        valid.map(str::to_owned)
    }

    fn disable(
        &mut self,
        table: &DeTable<'_>,
        disable: &Spanned<DeValue<'_>>,
        name: String,
    ) -> Option<Read> {
        let at = disable.span().start;
        let skip = |reader: &mut Self, why: &str| {
            let message = format_args!("rule {}: {why}, skipped", Quoted(&name));
            reader.problem(at, message);
        };
        if !matches!(disable.get_ref(), DeValue::Boolean(true)) {
            skip(self, "disable takes only true");
            return None;
        }
        if self.source != Source::Local {
            skip(self, "only the local file disables rules");
            return None;
        }
        if table.len() > 2 {
            skip(self, "a rule that disables takes only its name");
            return None;
        }
        Some(Read::Disable(name))
    }

    fn target(
        &mut self,
        state: &Spanned<DeValue<'_>>,
        reason: Option<&Spanned<DeValue<'_>>>,
        visible: Option<&Spanned<DeValue<'_>>>,
        name: &str,
    ) -> Option<Target> {
        let mut skip = |at: usize, why: &str| {
            let message = format_args!("rule {}: {why}, skipped", Quoted(name));
            self.problem(at, message);
            None
        };
        let visible = match visible.map(|value| (value, value.get_ref())) {
            None => None,
            Some((_, DeValue::Boolean(visible))) => Some(*visible),
            Some((value, _)) => return skip(value.span().start, "visible takes true or false"),
        };
        let target = match string(state.get_ref()) {
            Some("needs_input") => {
                let Some(reason) = reason else {
                    return skip(state.span().start, "needs_input needs a reason");
                };
                match string(reason.get_ref()).and_then(Reason::parse) {
                    Some(reason) => Target::NeedsInput(reason),
                    None => {
                        let why =
                            "reason must be permission, question, plan, elicitation or dialog";
                        return skip(reason.span().start, why);
                    }
                }
            }
            Some("working") => Target::Working,
            Some("idle") => Target::Idle {
                visible: visible.unwrap_or(false),
            },
            Some("hold") => Target::Hold,
            _ => {
                let why = "state must be needs_input, working, idle or hold";
                return skip(state.span().start, why);
            }
        };
        if let (Some(reason), false) = (reason, matches!(target, Target::NeedsInput(_))) {
            return skip(reason.span().start, "only needs_input takes a reason");
        }
        if visible.is_some() && !matches!(target, Target::Idle { .. }) {
            return skip(state.span().start, "only idle takes visible");
        }
        Some(target)
    }

    /// The matchers of `table`, at `depth`, counting them in `matchers`. At depth 0, the rule's
    /// own keys are passed over.
    fn test(
        &mut self,
        table: &DeTable<'_>,
        depth: usize,
        matchers: &mut usize,
        name: &str,
    ) -> Option<Test> {
        let mut test = Test::default();
        for (key, value) in table {
            let key_name = key.get_ref().as_ref();
            let at = value.span().start;
            match key_name {
                "contains" => {
                    for part in self.strings(value, matchers, name)? {
                        test.contains.push(part.to_ascii_lowercase());
                    }
                }
                "regex" | "line_regex" => {
                    let mut compiled = Vec::new();
                    for (pattern, at) in self.strings_at(value, matchers, name)? {
                        compiled.push(self.compile(pattern, at, name, SKIPPED)?);
                    }
                    if key_name == "regex" {
                        test.regex.extend(compiled);
                    } else {
                        test.line_regex.extend(compiled);
                    }
                }
                "any" | "all" | "not" => {
                    let inner = self.tables(value, depth, matchers, name)?;
                    match key_name {
                        "any" => test.any.extend(inner),
                        "all" => test.all.extend(inner),
                        _ => test.not.extend(inner),
                    }
                }
                other if depth == 0 && RULE_KEYS.contains(&other) => {}
                other => {
                    let message = format_args!(
                        "rule {}: unknown name {}, skipped",
                        Quoted(name),
                        Quoted(other)
                    );
                    self.problem(key.span().start, message);
                    return None;
                }
            }
            if *matchers > MAX_MATCHERS {
                let message = format_args!(
                    "rule {}: more than {MAX_MATCHERS} matchers, skipped",
                    Quoted(name)
                );
                self.problem(at, message);
                return None;
            }
        }
        Some(test)
    }

    /// The tables of `any`, `all` or `not`, one level below `depth`.
    fn tables(
        &mut self,
        value: &Spanned<DeValue<'_>>,
        depth: usize,
        matchers: &mut usize,
        name: &str,
    ) -> Option<Vec<Test>> {
        let at = value.span().start;
        let skip = |reader: &mut Self, why: fmt::Arguments<'_>| {
            reader.problem(at, format_args!("rule {}: {why}, skipped", Quoted(name)));
            None
        };
        if depth + 1 > MAX_DEPTH {
            return skip(
                self,
                format_args!("any, all and not nest at most {MAX_DEPTH} deep"),
            );
        }
        let DeValue::Array(array) = value.get_ref() else {
            return skip(self, format_args!("any, all and not take a list of tables"));
        };
        let mut tests = Vec::new();
        for entry in array {
            *matchers += 1;
            let DeValue::Table(table) = entry.get_ref() else {
                return skip(self, format_args!("any, all and not take a list of tables"));
            };
            let test = self.test(table, depth + 1, matchers, name)?;
            if test.is_empty() {
                return skip(self, format_args!("a table in any, all or not is empty"));
            }
            tests.push(test);
        }
        Some(tests)
    }

    fn strings<'v>(
        &mut self,
        value: &'v Spanned<DeValue<'_>>,
        matchers: &mut usize,
        name: &str,
    ) -> Option<Vec<&'v str>> {
        let found = self.strings_at(value, matchers, name)?;
        Some(found.into_iter().map(|(text, _)| text).collect())
    }

    /// A string, or a list of strings, each with where it is, none empty or too long.
    fn strings_at<'v>(
        &mut self,
        value: &'v Spanned<DeValue<'_>>,
        matchers: &mut usize,
        name: &str,
    ) -> Option<Vec<(&'v str, usize)>> {
        let entries: Vec<&Spanned<DeValue<'_>>> = match value.get_ref() {
            DeValue::Array(array) => array.iter().collect(),
            _ => vec![value],
        };
        let mut found = Vec::new();
        for entry in entries {
            *matchers += 1;
            let at = entry.span().start;
            match string(entry.get_ref()) {
                Some(text) if !text.is_empty() && text.len() <= MAX_PATTERN_BYTES => {
                    found.push((text, at));
                }
                _ => {
                    let message = format_args!(
                        "rule {}: each string must be 1 to {MAX_PATTERN_BYTES} bytes, skipped",
                        Quoted(name)
                    );
                    self.problem(at, message);
                    return None;
                }
            }
        }
        Some(found)
    }

    /// `pattern` compiled, or `None`, said: what then is not used is `skipped`.
    fn compile(&mut self, pattern: &str, at: usize, name: &str, skipped: &str) -> Option<Regex> {
        self.patterns += 1;
        if self.patterns > MAX_PATTERNS {
            let message = format_args!(
                "rule {}: more than {MAX_PATTERNS} patterns in the file, {skipped}",
                Quoted(name)
            );
            self.problem(at, message);
            return None;
        }
        let built = RegexBuilder::new(pattern)
            .size_limit(PATTERN_SIZE_LIMIT)
            .dfa_size_limit(PATTERN_SIZE_LIMIT)
            .build();
        match built {
            Ok(regex) => Some(regex),
            Err(error) => {
                let first = error.to_string();
                let first = first.lines().last().unwrap_or_default().trim().to_owned();
                let message = format_args!(
                    "rule {}: a pattern does not compile ({}), {skipped}",
                    Quoted(name),
                    Quoted(&first)
                );
                self.problem(at, message);
                None
            }
        }
    }
}

fn string<'v>(value: &'v DeValue<'_>) -> Option<&'v str> {
    match value {
        DeValue::String(text) => Some(text),
        _ => None,
    }
}

/// A decimal integer's digits, as written.
fn integer<'v>(value: &'v DeValue<'_>) -> Option<&'v str> {
    match value {
        DeValue::Integer(integer) if integer.radix() == 10 => Some(integer.as_str()),
        _ => None,
    }
}

/// Text from the file in a problem: in quotes, cut, with control characters escaped.
struct Quoted<'t>(&'t str);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut end = self.0.len().min(MAX_QUOTED);
        while !self.0.is_char_boundary(end) {
            end -= 1;
        }
        write!(f, "{:?}", &self.0[..end])?;
        if end < self.0.len() {
            f.write_str("…")?;
        }
        Ok(())
    }
}

/// Drops parsed tables a level at a time, so that deep nesting needs no deep stack. Its twin is
/// `tear_down` in `config/mod.rs`, which this module may not use (ADR 0121): change both.
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
