//! One component of a project's search (ADR 0114, ADR 0116): what it was asked to be, by its
//! affix, its text and the migemo setting, and compiling it, or keeping what a component of
//! the input before compiled.
//!
//! Must not depend on: `complete`, `editor` or file I/O.

use std::sync::Arc;

use regex::bytes::{Regex, RegexBuilder};

use super::LineQuery;
use crate::migemo::{self, Budget, Dictionary, Spent, Table};

/// The most memory a component's expression may take once compiled, as in completion.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// The deepest an expression may nest, as in completion.
const REGEX_NEST_LIMIT: u32 = 32;

/// The most strings a migemo component's pattern is kept as, besides its words, for a longer
/// one to be checked against: past them, a search for a longer one starts again.
pub(super) const MAX_ALTERNATIVES: usize = 1024;

/// The characters that make a component an expression, as well as a literal, as in completion.
const REGEX_META: &[char] = &[
    '\\', '.', '+', '*', '?', '(', ')', '|', '[', ']', '{', '}', '^', '$',
];

/// One component of a search.
#[derive(Clone)]
pub(super) struct Component {
    pub(super) regex: Regex,
    /// A line must not hold it.
    pub(super) not: bool,
    /// What it was asked to be, which decides the rest.
    pub(super) asked: Asked,
    /// What it matches as.
    pub(super) kind: Kind,
    /// What was typed, without its affix, in lower case where case is ignored.
    pub(super) pattern: Box<str>,
    /// For a migemo one, what its words spent of the input's budget, which decides them.
    pub(super) spent: Spent,
    /// For one that matches as migemo's pattern, every string the pattern matches, if they
    /// are at most [`MAX_ALTERNATIVES`] besides its words: a line it matches holds one.
    pub(super) alternatives: Option<Strings>,
    /// It was compiled for this query, not kept from the one before, for tests.
    #[cfg(test)]
    pub(super) fresh: bool,
}

/// The strings a migemo component's pattern matches, shared by the components that keep it.
pub(super) type Strings = Arc<[Box<str>]>;

/// What a component was asked to be, by its text, its affix and the setting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Asked {
    /// `=` or no character an expression reads, and not romaji that gives kana.
    Literal,
    /// A literal, and an expression too if it is one alone.
    Expression,
    /// Migemo's pattern, with the words of the dictionary of this generation, if any: only
    /// its generation is kept, so a query never keeps a table alive.
    Migemo(Option<u64>),
}

/// What a component matches as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// The text as typed.
    Literal,
    /// The literal or an expression, so a longer one may match more.
    Expression,
    /// Migemo's pattern, whose words a longer string may not hold.
    Migemo,
}

/// `pattern` as an expression that matches where it does as an expression or as a literal, or as
/// a literal alone, ignoring case if `ignore_case`, and whether it was read as an expression too.
/// `None` if not even the literal compiles within the limits.
///
/// The expression must parse alone: joined to the literal, one that is no expression alone, as
/// `a)|(b`, can compile and match what neither does. A `(?x)` comment would run past the
/// expression's group, so the group is then ended by a line break, which `(?x)` ignores; the
/// literal writes its own line breaks as escapes, so that no comment ends inside it.
pub(super) fn compile(pattern: &str, expression: bool, ignore_case: bool) -> Option<(Regex, bool)> {
    let build = |source: &str, size_limit: usize| {
        RegexBuilder::new(source)
            .case_insensitive(ignore_case)
            .multi_line(true)
            .crlf(true)
            .size_limit(size_limit)
            .nest_limit(REGEX_NEST_LIMIT)
            .build()
    };
    let literal = regex::escape(pattern)
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    // With no room to compile in, a build stops once the expression has parsed.
    let parses = || !matches!(build(pattern, 0), Err(regex::Error::Syntax(_)));
    if expression && parses() {
        let joined = build(&format!("(?:{pattern})|(?:{literal})"), REGEX_SIZE_LIMIT)
            .ok()
            .or_else(|| {
                pattern.contains('#').then(|| {
                    build(&format!("(?:{pattern}\n)|(?:{literal})"), REGEX_SIZE_LIMIT).ok()
                })?
            });
        if let Some(regex) = joined {
            return Some((regex, true));
        }
    }
    build(&literal, REGEX_SIZE_LIMIT)
        .ok()
        .map(|regex| (regex, false))
}

/// Migemo's pattern for `string`, ignoring case if `ignore_case`, with as many of
/// `dictionary`'s words as `budget` leaves, and the strings it matches: `None` if it gives no
/// kana and is matched as a literal.
fn compile_migemo(
    string: &str,
    ignore_case: bool,
    dictionary: Option<&Dictionary>,
    budget: Budget,
) -> (Option<(Regex, Option<Strings>)>, Spent) {
    let compiled = migemo::compile_within(
        string,
        dictionary,
        migemo::WORDS_SIZE_LIMIT,
        budget,
        |pattern, size_limit| {
            RegexBuilder::new(pattern)
                .case_insensitive(ignore_case)
                .size_limit(size_limit)
                .nest_limit(migemo::NEST_LIMIT)
                .build()
                .ok()
        },
    );
    let regex = compiled.compiled.map(|regex| {
        let alternatives = migemo::alternatives(string, &compiled.words, MAX_ALTERNATIVES);
        (regex, alternatives.map(Arc::from))
    });
    (regex, compiled.spent)
}

impl Component {
    /// The component `raw` stands for, with its affix read, reusing what a component of
    /// `before`, or one `built` before it of the same input, compiled for the same text and
    /// request, or `None` if nothing is left to match or not even its literal compiles.
    /// `migemo` is the setting, and `table` the dictionary. A migemo one spends `budget`, which
    /// the input's components share, and one kept spent what it would now.
    pub(super) fn parse(
        raw: &str,
        ignore_case: bool,
        (before, built): (&LineQuery, &[Self]),
        (migemo, table): (bool, Option<&Table>),
        budget: &mut Budget,
    ) -> Option<Self> {
        let (not, literal, rest) = match raw.chars().next() {
            Some('!') => (true, false, &raw[1..]),
            Some('=') => (false, true, &raw[1..]),
            _ => (false, false, raw),
        };
        if rest.is_empty() {
            return None;
        }
        let asked = if literal {
            Asked::Literal
        } else if migemo && migemo::gives_kana(rest) {
            Asked::Migemo(migemo::generation(table))
        } else if rest.contains(REGEX_META) {
            Asked::Expression
        } else {
            Asked::Literal
        };
        let pattern: Box<str> = if ignore_case {
            rest.to_lowercase().into()
        } else {
            rest.into()
        };
        // What is excluded compiles as what is not, so either is kept for the other.
        let same = (before.components.iter())
            .filter(|_| before.ignore_case == ignore_case)
            .chain(built)
            .find(|old| {
                old.asked == asked && old.pattern == pattern && old.spent.same_within(*budget)
            });
        if let Some(old) = same {
            budget.spend(old.spent.spent);
            return Some(Self {
                regex: old.regex.clone(),
                not,
                asked,
                kind: old.kind,
                pattern,
                spent: old.spent,
                alternatives: old.alternatives.clone(),
                #[cfg(test)]
                fresh: false,
            });
        }
        let (migemo, spent) = match &asked {
            Asked::Migemo(_) => {
                let dictionary = table.map(|table| &*table.dictionary);
                compile_migemo(rest, ignore_case, dictionary, *budget)
            }
            _ => (None, Spent::default()),
        };
        budget.spend(spent.spent);
        let (regex, kind, alternatives) = match migemo {
            Some((regex, alternatives)) => (regex, Kind::Migemo, alternatives),
            None => {
                let (regex, expression) = compile(rest, asked == Asked::Expression, ignore_case)?;
                let kind = if expression {
                    Kind::Expression
                } else {
                    Kind::Literal
                };
                (regex, kind, None)
            }
        };
        Some(Self {
            regex,
            not,
            asked,
            kind,
            pattern,
            spent,
            alternatives,
            #[cfg(test)]
            fresh: true,
        })
    }
}
