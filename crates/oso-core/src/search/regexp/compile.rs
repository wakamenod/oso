//! Compiling a regexp (ADR 0113, ADR 0116): smart case, which looks past escapes, the
//! limits on its length and size, and what is wrong with one that does not compile, with
//! Emacs's `incomplete input` for one that is still being typed.
//!
//! Must not depend on: `buffer`, `editor` or a log. No error says the pattern itself.

use std::cell::OnceCell;
use std::sync::Arc;

use regex::{Regex, RegexBuilder};

use super::cut::AtCut;
use super::{MAX_LINE, MAX_PATTERN_CHARS, Regexp, SIZE_LIMIT};

/// Why a pattern did not compile for isearch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Invalid {
    /// It ends before what it opened is closed, as a `[` being typed: more of it may compile.
    Incomplete,
    /// It is larger than the limit it was compiled with, which a larger one may take.
    TooBig,
    /// What is wrong with it, on one line.
    Wrong(String),
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

/// Compiles `pattern` for isearch, case-insensitive if `insensitive`, within `limit` bytes
/// at most [`SIZE_LIMIT`]: the regexps for a cut wait until a search needs them, unless
/// `eager`, as for a compile on the worker.
///
/// # Errors
///
/// [`Invalid::Incomplete`] while what it opened is not closed, [`Invalid::TooBig`] past a
/// `limit` under [`SIZE_LIMIT`], and otherwise what is wrong with it.
pub(crate) fn compile_isearch(
    pattern: &str,
    insensitive: bool,
    (limit, eager): (usize, bool),
) -> Result<Regexp, Invalid> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(Invalid::Wrong(too_long()));
    }
    let limit = limit.min(SIZE_LIMIT);
    let mut built = builder(pattern, insensitive);
    match built.size_limit(limit).build() {
        Ok(regex) => {
            let regexp = Regexp::new(regex, pattern, insensitive);
            if eager {
                regexp.at_cut();
            }
            Ok(regexp)
        }
        Err(regex::Error::CompiledTooBig(_)) if limit < SIZE_LIMIT => Err(Invalid::TooBig),
        Err(error) if incomplete(&error) => Err(Invalid::Incomplete),
        Err(error) => Err(Invalid::Wrong(why(&error))),
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
    fn new(regex: Regex, pattern: &str, insensitive: bool) -> Self {
        Self {
            regex,
            at_cut: OnceCell::new(),
            source: Arc::new((pattern.to_owned(), insensitive)),
            longest: MAX_LINE,
        }
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
