//! query-replace's regexps (ADR 0113): the `regex` crate's syntax, matched one line at a time
//! over a rope, with smart case, and a replacement's `$1`, `${name}` and `$0`.
//!
//! Must not depend on: `buffer`, `editor` or a log. `Debug` prints no pattern, which can say
//! something of a buffer that may hold secrets.

use std::fmt;
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use super::Found;
use crate::rope::Rope;

/// The longest line a regexp is matched in: past it, a search stops at the line.
pub(crate) const MAX_LINE: usize = 64 * 1024 * 1024;

/// How much a search reads between looks at whether to stop.
const LOOK_EVERY: usize = 64 * 1024;

/// A line too long to match a regexp in, which starts at this offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineTooLong(pub(crate) usize);

/// A regexp compiled with smart case.
#[derive(Clone)]
pub(crate) struct Regexp {
    regex: Regex,
}

impl fmt::Debug for Regexp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Regexp").finish_non_exhaustive()
    }
}

/// Whether `pattern` makes the search case-sensitive: it holds an uppercase letter outside an
/// escape, as Emacs looks at a regexp: `\W` and `\p{Lu}` hold none, and `Foo` does.
pub(crate) fn smart_case(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // `\p{…}` and `\P{…}` name a class: skip to its end.
            if let Some('p' | 'P') = chars.next() {
                let mut rest = chars.clone();
                if rest.next() == Some('{') {
                    for c in rest.by_ref() {
                        if c == '}' {
                            break;
                        }
                    }
                    chars = rest;
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
/// What is wrong with it, in the crate's words, on one line.
pub(crate) fn compile(pattern: &str) -> Result<Regexp, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(!smart_case(pattern))
        .multi_line(false)
        .build()
        .map(|regex| Regexp { regex })
        .map_err(|error| {
            let error = error.to_string();
            let last = error.lines().last().unwrap_or("invalid regexp").trim();
            format!("Invalid regexp: {last}")
        })
}

/// The text of `range`, which holds no line break.
fn line_text(text: &Rope, range: Range<usize>, into: &mut String) {
    into.clear();
    for chunk in text.chunks_in_range(range) {
        into.push_str(chunk);
    }
}

impl Regexp {
    /// The first match within `range`, each line its own haystack, reading at most `limit`
    /// bytes, and none past `stop`: a line that would take it past `limit` is not read. A match never crosses a line break, `^` and `$`
    /// are a line's start and end, and a match ending past `range` is not one.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] at a line longer than [`MAX_LINE`].
    pub(crate) fn find_in(
        &self,
        text: &Rope,
        range: Range<usize>,
        limit: usize,
        stop: &dyn Fn() -> bool,
    ) -> Result<Found, LineTooLong> {
        let first = text.offset_to_point(range.start).row;
        let last = text.offset_to_point(range.end).row;
        let mut line = String::new();
        let (mut read, mut unchecked) = (0usize, 0usize);
        for row in first..=last {
            let (start, end) = (text.line_start(row), text.line_end(row));
            if end - start > MAX_LINE {
                return Err(LineTooLong(start));
            }
            let resume = start.max(range.start);
            // A line is read whole, so one that would pass the budget is left for later.
            if read.saturating_add(end - start) > limit || (unchecked > LOOK_EVERY && stop()) {
                return Ok(Found::Stopped { resume });
            }
            if unchecked > LOOK_EVERY {
                unchecked = 0;
            }
            line_text(text, start..end, &mut line);
            let from = range.start.saturating_sub(start).min(line.len());
            if let Some(found) = self.regex.find_at(&line, from) {
                let found = start + found.start()..start + found.end();
                if found.end <= range.end {
                    return Ok(Found::Match(found));
                }
                return Ok(Found::None);
            }
            read += end - start + 1;
            unchecked += end - start + 1;
        }
        Ok(Found::None)
    }

    /// The replacement for the match at `range`: `to` with its `$1`, `${name}`, `$0` and `$$`
    /// expanded from the match's groups, as `Regex::replace` expands them.
    pub(crate) fn expand(&self, text: &Rope, range: &Range<usize>, to: &str) -> String {
        let row = text.offset_to_point(range.start).row;
        let (start, end) = (text.line_start(row), text.line_end(row));
        let mut line = String::new();
        line_text(text, start..end.max(range.end), &mut line);
        let mut out = String::new();
        if let Some(captures) = self.regex.captures_at(&line, range.start - start) {
            captures.expand(to, &mut out);
        } else {
            out.push_str(to);
        }
        out
    }

    /// The matches within `spans`, at most `most`, reading the lines they are on if each is
    /// at most `longest` bytes, and no more than `limit` bytes in all: for the highlights of
    /// a frame.
    pub(crate) fn matches_in(
        &self,
        text: &Rope,
        spans: &[Range<usize>],
        (most, limit, longest): (usize, usize, usize),
        out: &mut Vec<Range<usize>>,
    ) {
        let mut line = String::new();
        let mut read = 0;
        let mut last_row = None;
        for span in spans {
            let first = text.offset_to_point(span.start).row;
            let last = text.offset_to_point(span.end).row;
            for row in first..=last {
                if last_row.is_some_and(|done| row <= done) {
                    continue;
                }
                last_row = Some(row);
                let (start, end) = (text.line_start(row), text.line_end(row));
                if end - start > longest || read + (end - start) > limit {
                    continue;
                }
                read += end - start;
                line_text(text, start..end, &mut line);
                let mut at = 0;
                while let Some(found) = self.regex.find_at(&line, at) {
                    let range = start + found.start()..start + found.end();
                    if range.end > span.start && range.start < span.end && !range.is_empty() {
                        out.push(range);
                        if out.len() == most {
                            return;
                        }
                    }
                    at = if found.end() > found.start() {
                        found.end()
                    } else {
                        line[found.end()..]
                            .chars()
                            .next()
                            .map_or(line.len() + 1, |c| found.end() + c.len_utf8())
                    };
                    if at > line.len() {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pattern: &str, text: &str, from: usize) -> Found {
        let rope = Rope::from(text);
        compile(pattern)
            .unwrap()
            .find_in(&rope, from..rope.len(), usize::MAX, &|| false)
            .unwrap()
    }

    #[test]
    fn smart_case_looks_past_escapes() {
        assert!(!smart_case(r"\W+\p{Lu}foo"));
        assert!(smart_case("Foo"));
        assert!(smart_case(r"\dA"));
        assert_eq!(find("foo", "x FOO", 0), Found::Match(2..5));
        assert_eq!(find("Foo", "x FOO Foo", 0), Found::Match(6..9));
        assert_eq!(find("(?-i)foo", "FOO", 0), Found::None);
    }

    #[test]
    fn a_match_keeps_to_its_line() {
        assert_eq!(find("^b", "ab\nbc", 0), Found::Match(3..4));
        assert_eq!(find("b$", "ab\nbc", 0), Found::Match(1..2));
        assert_eq!(find(r"b\nb", "ab\nbc", 0), Found::None);
        assert_eq!(find("c", "ab\nbc", 4), Found::Match(4..5));
    }

    #[test]
    fn an_invalid_regexp_says_why() {
        let error = compile("(").unwrap_err();
        assert!(error.starts_with("Invalid regexp: "), "{error}");
    }

    #[test]
    fn groups_expand_in_the_replacement() {
        let rope = Rope::from("x key=value");
        let regexp = compile(r"(\w+)=(?P<v>\w+)").unwrap();
        assert_eq!(regexp.expand(&rope, &(2..11), "${v}:$1 $$"), "value:key $");
    }

    #[test]
    fn highlights_take_the_matches_in_view() {
        let rope = Rope::from("a1 a2\nb3 a4\n");
        let regexp = compile(r"a\d").unwrap();
        let mut out = Vec::new();
        let spans: Vec<_> = std::iter::once(0..rope.len()).collect();
        regexp.matches_in(&rope, &spans, (10, 1000, 1000), &mut out);
        assert_eq!(out, [0..2, 3..5, 9..11]);
    }
}
