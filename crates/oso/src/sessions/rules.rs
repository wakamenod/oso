//! The rules that tell a hosted session's state from its screen (ADR 0121, Q3, Q4): the bundled
//! set, compiled into oso, and the person's local file, which replaces bundled rules by name and
//! disables them. Never read from a project, never fetched.
//!
//! Must not depend on: `app`, `dispatch` or `config`; `config`'s thread hands this
//! the local file's text, and this compiles it there.
//!
//! The format is in `docs/dev/agent-rules.md`. Every pattern is the `regex` crate's (ADR
//! 0010), compiled once per load with a size limit, and the counts of rules, matchers and
//! patterns are bounded, so that the work per screen is linear in its text and bounded.

mod parse;

use std::time::{Duration, Instant};

use regex::Regex;

use super::recap::Recap;
use super::screen::Region;
use super::state::Target;

#[cfg(test)]
pub use parse::MAX_PROBLEMS;
pub use parse::parse;

/// The rules compiled into oso.
pub const BUNDLED: &str = include_str!("claude.toml");

/// The local file's path under oso's configuration directory.
pub const LOCAL_FILE: &str = "agent-rules/claude.toml";

/// The most rules a file holds; the rest are skipped.
pub const MAX_RULES: usize = 128;

/// The most matchers in one rule: each string of `contains`, each pattern and each table of
/// `any`, `all` and `not`, at every depth.
pub const MAX_MATCHERS: usize = 64;

/// How deep `any`, `all` and `not` nest.
pub const MAX_DEPTH: usize = 4;

/// The most patterns in a file.
pub const MAX_PATTERNS: usize = 256;

/// The longest pattern or `contains` string, in bytes.
pub const MAX_PATTERN_BYTES: usize = 4096;

/// The size limit of one compiled pattern, and of its lazy DFA's cache (ADR 0121).
pub const PATTERN_SIZE_LIMIT: usize = 128 << 10;

/// Where a rule comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Bundled,
    Local,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::Local => "local",
        }
    }
}

/// One rule: when its test holds over its region, the session is in its target.
#[derive(Debug)]
pub struct Rule {
    pub name: String,
    pub target: Target,
    pub priority: i64,
    pub region: Region,
    pub source: Source,
    /// What the recap of a session in the rule's state shows (ADR 0121, step 3).
    pub recap: Option<Recap>,
    test: Test,
}

impl Rule {
    /// Whether the rule's test holds over `text`, its region's text, and `lower`, the same
    /// text with ASCII letters in lower case; [`Late`] if `deadline` passed before it was known.
    pub fn holds(&self, text: &str, lower: &str, deadline: &Deadline) -> Result<bool, Late> {
        self.test.holds(text, lower, deadline)
    }
}

/// What a rule or one of its `any`, `all` or `not` tables checks. Every part must hold.
#[derive(Debug, Default)]
struct Test {
    /// Strings the text holds, in lower case: matched with ASCII case ignored.
    contains: Vec<String>,
    /// Patterns that match somewhere in the text.
    regex: Vec<Regex>,
    /// Patterns that each match some line of the text.
    line_regex: Vec<Regex>,
    /// At least one holds, if there are any.
    any: Vec<Test>,
    /// Every one holds.
    all: Vec<Test>,
    /// None holds.
    not: Vec<Test>,
}

impl Test {
    fn is_empty(&self) -> bool {
        self.contains.is_empty()
            && self.regex.is_empty()
            && self.line_regex.is_empty()
            && self.any.is_empty()
            && self.all.is_empty()
            && self.not.is_empty()
    }

    /// Whether every part holds, the deadline looked at before each pattern and each table.
    fn holds(&self, text: &str, lower: &str, deadline: &Deadline) -> Result<bool, Late> {
        deadline.check()?;
        if !self
            .contains
            .iter()
            .all(|part| lower.contains(part.as_str()))
        {
            return Ok(false);
        }
        for pattern in &self.regex {
            deadline.check()?;
            if !pattern.is_match(text) {
                return Ok(false);
            }
        }
        for pattern in &self.line_regex {
            deadline.check()?;
            if !text.lines().any(|line| pattern.is_match(line)) {
                return Ok(false);
            }
        }
        if !self.any.is_empty() {
            let mut one = false;
            for test in &self.any {
                if test.holds(text, lower, deadline)? {
                    one = true;
                    break;
                }
            }
            if !one {
                return Ok(false);
            }
        }
        for test in &self.all {
            if !test.holds(text, lower, deadline)? {
                return Ok(false);
            }
        }
        for test in &self.not {
            if test.holds(text, lower, deadline)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// How long the rules may take over one look at a screen (ADR 0121, as built): past it, the
/// state holds. Only a crafted local file comes near it.
pub const BUDGET: Duration = Duration::from_millis(5);

/// When the rules over one look must have given their verdict.
#[derive(Clone, Copy, Debug)]
pub struct Deadline {
    at: Instant,
}

/// The deadline passed before a rule's test was known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Late;

impl Deadline {
    /// The deadline `budget` from now.
    pub fn after(budget: Duration) -> Self {
        Self {
            at: Instant::now() + budget,
        }
    }

    pub(super) fn check(&self) -> Result<(), Late> {
        if Instant::now() >= self.at {
            return Err(Late);
        }
        Ok(())
    }
}

/// A rules file, read: its rules in its order, the names it disables, and what is wrong in it.
#[derive(Debug, Default)]
pub struct Parsed {
    pub rules: Vec<Rule>,
    pub disabled: Vec<String>,
    /// What is wrong, `LINE: message`, in the order of the file: the first
    /// [`MAX_PROBLEMS`](parse::MAX_PROBLEMS).
    pub problems: Vec<String>,
    /// How many problems there were after those.
    pub more: usize,
    /// The whole file was refused: a syntax error or an unknown version. Nothing in it applies.
    pub refused: bool,
}

/// The rules in force: the bundled ones, less those the local file disables or replaces, and
/// the local file's, highest priority first; rules of one priority keep their order, bundled
/// before local.
#[derive(Debug, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
    /// How many came from each source, and how many bundled rules the local file disabled.
    pub bundled: usize,
    pub local: usize,
    pub disabled: usize,
}

impl RuleSet {
    /// The bundled rules alone. Their problems are logged; a test keeps them at none.
    pub fn bundled() -> Self {
        Self::merged(bundled(), Parsed::default())
    }

    /// `bundled`, with `local` laid over it.
    pub fn merged(bundled: Parsed, local: Parsed) -> Self {
        let is_disabled = |name: &str| local.disabled.iter().any(|n| n == name);
        let replaced = |name: &str| local.rules.iter().any(|r| r.name == name);
        let disabled = bundled
            .rules
            .iter()
            .filter(|rule| is_disabled(&rule.name))
            .count();
        let mut rules: Vec<Rule> = bundled
            .rules
            .into_iter()
            .filter(|rule| !is_disabled(&rule.name) && !replaced(&rule.name))
            .collect();
        let kept = rules.len();
        let local_count = local.rules.len();
        rules.extend(local.rules);
        // A stable sort keeps the files' order within a priority.
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.priority));
        Self {
            rules,
            bundled: kept,
            local: local_count,
            disabled,
        }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
}

/// The bundled rules, parsed.
pub fn bundled() -> Parsed {
    let parsed = parse(BUNDLED, Source::Bundled);
    for problem in &parsed.problems {
        log::error!("bundled agent rules: {problem}");
    }
    parsed
}

#[cfg(test)]
mod tests;
