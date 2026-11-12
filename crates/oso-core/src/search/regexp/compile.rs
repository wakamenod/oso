//! Compiling a regexp (ADR 0113, ADR 0116): smart case, which looks past escapes, the
//! limits on its length and size, which patterns an isearch key compiles on the UI thread,
//! and what is wrong with one that does not compile, with Emacs's `incomplete input` for one
//! that is still being typed.
//!
//! Must not depend on: `buffer`, `editor` or a log. No error says the pattern itself.

use std::sync::Arc;

use regex::{Regex, RegexBuilder};

use super::cut::AtCut;
use super::{MAX_LINE, MAX_PATTERN_CHARS, Regexp, SIZE_LIMIT};

/// Why a pattern did not compile for isearch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CompileError {
    /// It ends before what it opened is closed, as a `[` being typed: more of it may compile.
    Incomplete,
    /// It may take longer to compile than a key has, and the worker compiles it.
    ForWorker,
    /// What is wrong with it, on one line.
    Invalid(String),
}

/// Where an isearch's pattern is compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum On {
    /// On a key, on the UI thread: within [`KEY_SIZE_LIMIT`](super::KEY_SIZE_LIMIT), and
    /// with at most one large Unicode class.
    Key,
    /// On the worker: within [`SIZE_LIMIT`].
    Worker,
}

/// How long a key's compile may take for the key to build the regexps for a cut too, which
/// take about twice as long.
const QUICK: std::time::Duration = std::time::Duration::from_micros(500);

/// The most weight a key compiles: case folding a class as large as `\p{Any}` takes about
/// 1.4 ms, and every part of a pattern that weighs nothing far less.
const KEY_WEIGHT: usize = 2;

/// What a bracket class weighs that holds a negated Perl class (`[\s\S]`), a Unicode class or
/// code point escape, or a character that is not ASCII, as a range to one: as much as 2.8 ms
/// each, so one is the worker's.
const HEAVY_CLASS: usize = KEY_WEIGHT + 1;

/// Whether `pattern` may take a key too long to compile, for the large Unicode classes case
/// folding goes through: outside brackets, `\p` and `\P` weigh two and a code point escape
/// (`\x`, `\u`, `\U`) one, as a range needs two, and a bracket class that may be large
/// weighs [`HEAVY_CLASS`].
fn heavy(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    let (mut weight, mut depth) = (0, 0usize);
    while let Some(c) = chars.next() {
        let inside = depth > 0;
        weight += match c {
            '[' => {
                depth += 1;
                0
            }
            ']' => {
                depth = depth.saturating_sub(1);
                0
            }
            '\\' => match (chars.next(), inside) {
                (Some('S' | 'W' | 'D' | 'p' | 'P' | 'x' | 'u' | 'U'), true) => HEAVY_CLASS,
                (Some('p' | 'P'), false) => 2,
                (Some('x' | 'u' | 'U'), false) => 1,
                _ => 0,
            },
            c if inside && !c.is_ascii() => HEAVY_CLASS,
            _ => 0,
        };
        if weight > KEY_WEIGHT {
            return true;
        }
    }
    false
}

/// How many characters of an escape follow `\` and its letter: the hex digits of `\x41`,
/// `A` and `\U00000041`, or one for a class named by one letter, as `\pL`.
fn escape_tail(letter: char) -> usize {
    match letter {
        'x' => 2,
        'u' => 4,
        'U' => 8,
        'p' | 'P' => 1,
        _ => 0,
    }
}

/// Whether `pattern` makes the search case-sensitive: it holds an uppercase letter outside an
/// escape, as Emacs looks at a regexp: `\W`, `\pL`, `\p{Lu}` and `\x{4A}` hold none, and `Foo`
/// does.
pub(crate) fn smart_case(pattern: &str) -> bool {
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        // A group's name, `(?P<Name>…)` or `(?<Name>…)`, says nothing of case.
        if c == '(' && chars.peek() == Some(&'?') {
            let mut rest = chars.clone();
            rest.next();
            if rest.peek() == Some(&'P') {
                rest.next();
            }
            if rest.next() == Some('<') {
                for c in rest.by_ref() {
                    if c == '>' {
                        break;
                    }
                }
                chars = rest;
            }
            continue;
        }
        if c == '\\' {
            let Some(letter) = chars.next() else {
                break;
            };
            if chars.peek() == Some(&'{') && escape_tail(letter) > 0 {
                // `\p{…}`, `\x{…}`: to the brace that closes it.
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                }
            } else {
                for _ in 0..escape_tail(letter) {
                    chars.next();
                }
            }
        } else if c.is_uppercase() {
            return true;
        }
    }
    false
}

/// Compiles `pattern`, case-insensitive unless it holds an uppercase letter outside an
/// escape; a `(?i)` or `(?-i)` in it wins.
///
/// # Errors
///
/// What is wrong with it, on one line: too long, or in the crate's words.
pub(crate) fn compile(pattern: &str) -> Result<Regexp, String> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(too_long());
    }
    let insensitive = !smart_case(pattern);
    let regexp = builder(pattern, insensitive)
        .build()
        .map(|regex| Regexp::new(regex, pattern, insensitive))
        .map_err(|error| why(&error))?;
    // The ones for a cut are part of it for query-replace, whose prompt says if they fail.
    if regexp.at_cut().is_none() {
        return Err(why(&regex::Error::CompiledTooBig(SIZE_LIMIT)));
    }
    Ok(regexp)
}

/// Compiles `pattern` for isearch, case-insensitive if `insensitive`, `on` a key or the
/// worker. The regexps for a cut wait until a search needs them, and a key builds them only
/// for a regexp it compiled quickly.
///
/// # Errors
///
/// [`CompileError::Incomplete`] while what it opened is not closed,
/// [`CompileError::ForWorker`] on a key for one larger or heavier than a key compiles, and
/// otherwise what is wrong with it.
pub(crate) fn compile_isearch(
    pattern: &str,
    insensitive: bool,
    on: On,
) -> Result<Regexp, CompileError> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(CompileError::Invalid(too_long()));
    }
    if on == On::Key && heavy(pattern) {
        return Err(CompileError::ForWorker);
    }
    let limit = match on {
        On::Key => super::KEY_SIZE_LIMIT,
        On::Worker => SIZE_LIMIT,
    };
    let mut built = builder(pattern, insensitive);
    let started = std::time::Instant::now();
    match built.size_limit(limit).build() {
        Ok(regex) => {
            let mut regexp = Regexp::new(regex, pattern, insensitive);
            regexp.quick = on == On::Key && started.elapsed() <= QUICK;
            Ok(regexp)
        }
        Err(regex::Error::CompiledTooBig(_)) if on == On::Key => Err(CompileError::ForWorker),
        Err(error) if incomplete(&error) => Err(CompileError::Incomplete),
        Err(error) => Err(CompileError::Invalid(why(&error))),
    }
}

/// What a pattern over [`MAX_PATTERN_CHARS`] says.
fn too_long() -> String {
    format!("Invalid regexp: longer than {MAX_PATTERN_CHARS} characters")
}

/// What is wrong with a pattern, on one line, in the crate's words.
fn why(error: &regex::Error) -> String {
    let error = error.to_string();
    let last = error.lines().last().unwrap_or("invalid regexp").trim();
    let last = last.strip_prefix("error:").unwrap_or(last).trim();
    format!("Invalid regexp: {last}")
}

/// Whether `error` says the pattern ended before what it opened was closed, as Emacs's
/// `Premature` and `Unmatched` errors, which isearch shows as `incomplete input`.
fn incomplete(error: &regex::Error) -> bool {
    let regex::Error::Syntax(text) = error else {
        return false;
    };
    let last = text.lines().last().unwrap_or_default();
    [
        "unclosed",
        "prematurely",
        "end of regex",
        "no closing brace",
    ]
    .iter()
    .any(|words| last.contains(words))
}

impl Regexp {
    #[expect(
        clippy::disallowed_types,
        reason = "a lazy part of one value, not global state (ADR 0038)"
    )]
    fn new(regex: Regex, pattern: &str, insensitive: bool) -> Self {
        Self {
            regex,
            at_cut: std::sync::OnceLock::new(),
            source: Arc::new((pattern.to_owned(), insensitive)),
            quick: false,
            longest: MAX_LINE,
        }
    }

    /// Builds the regexps for a match at a cut, if they are not built yet, where there is
    /// time: on the worker, or once as the keys that add cursors begin.
    pub(crate) fn build_cut(&self) {
        self.at_cut();
    }

    /// Whether the regexps for a cut are built, or found not to compile.
    pub(super) fn cut_ready(&self) -> bool {
        self.at_cut.get().is_some()
    }

    /// The regexps for a match at a cut, built the first time, `None` if they do not compile.
    pub(super) fn at_cut(&self) -> Option<&AtCut> {
        self.at_cut
            .get_or_init(|| {
                let (pattern, insensitive) = &*self.source;
                AtCut::new(pattern, *insensitive).ok()
            })
            .as_ref()
    }
}

/// A builder of `pattern` with query-replace's settings.
pub(super) fn builder(pattern: &str, insensitive: bool) -> RegexBuilder {
    let mut builder = RegexBuilder::new(pattern);
    builder
        .case_insensitive(insensitive)
        .multi_line(false)
        .size_limit(SIZE_LIMIT);
    builder
}
