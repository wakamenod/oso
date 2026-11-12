//! query-replace's regexps (ADR 0113): the `regex` crate's syntax, matched one line at a time
//! over a rope, with smart case, and a replacement's `$1`, `${name}` and `$0`.
//!
//! Must not depend on: `buffer`, `editor` or a log. `Debug` prints no pattern, which can say
//! something of a buffer that may hold secrets.
//!
//! - A search keeps the line it is in and goes on in it match after match, so a line is read
//!   once however many matches it holds, and what it costs is counted: each line read, and
//!   each look for a match from where the last ended to the line's end.
//! - Empty matches go as `Regex::replace_all` takes them: one right where a match ended is
//!   skipped, and one at the end of a range counts.

use std::fmt;
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use super::Found;
use crate::rope::Rope;

/// The longest line a regexp is matched in: past it, a search stops at the line.
pub(crate) const MAX_LINE: usize = 64 * 1024 * 1024;

/// The most characters of a regexp, as isearch's string (ADR 0082).
pub(crate) const MAX_PATTERN_CHARS: usize = 1024;

/// The most memory a compiled regexp takes.
const SIZE_LIMIT: usize = 1024 * 1024;

/// What reading a line costs besides its bytes, as a line's seek and copy take time of
/// their own.
const PER_LINE: usize = 64;

/// How much work a search does between looks at whether to stop.
const LOOK_EVERY: usize = 64 * 1024;

/// What a match found costs besides the text it spans.
const PER_MATCH: usize = 16;

/// How far past the view a frame's highlights look on a line.
const VIEW_REACH: usize = 256;

/// A line too long to match a regexp in, which starts at this offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineTooLong(pub(crate) usize);

/// A regexp compiled with smart case.
#[derive(Clone)]
pub(crate) struct Regexp {
    regex: Regex,
    /// The longest line it is matched in.
    longest: usize,
}

impl fmt::Debug for Regexp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Regexp").finish_non_exhaustive()
    }
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
        return Err(format!(
            "Invalid regexp: longer than {MAX_PATTERN_CHARS} characters"
        ));
    }
    RegexBuilder::new(pattern)
        .case_insensitive(!smart_case(pattern))
        .multi_line(false)
        .size_limit(SIZE_LIMIT)
        .build()
        .map(|regex| Regexp {
            regex,
            longest: MAX_LINE,
        })
        .map_err(|error| {
            let error = error.to_string();
            let last = error.lines().last().unwrap_or("invalid regexp").trim();
            let last = last.strip_prefix("error:").unwrap_or(last).trim();
            format!("Invalid regexp: {last}")
        })
}

/// A line's text, read from a rope, which a search keeps while it is in the line, and which
/// `!` keeps across its slices while the text does not change but by its own edits.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    /// Where it starts.
    pub(crate) start: usize,
    /// Where the next line starts, or past the end of the text for the last.
    pub(crate) next: usize,
    /// Its text, without its line break.
    pub(crate) text: String,
}

impl Line {
    /// The line `offset` is on.
    fn read(text: &Rope, offset: usize, longest: usize) -> Result<Self, LineTooLong> {
        let row = text.offset_to_point(offset).row;
        let (start, end) = (text.line_start(row), text.line_end(row));
        if end - start > longest {
            return Err(LineTooLong(start));
        }
        let next = if row >= text.summary().lines {
            text.len() + 1
        } else {
            text.line_start(row + 1)
        };
        let mut line = String::with_capacity(end - start);
        line.extend(text.chunks_in_range(start..end));
        Ok(Self {
            start,
            next,
            text: line,
        })
    }

    /// Whether `offset` is in it, its end included.
    fn holds(&self, offset: usize) -> bool {
        offset >= self.start && offset <= self.start + self.text.len()
    }

    /// The line after `edits`, ascending and apart and within it, each a range of it and its
    /// replacement: as the rope has them once they are made, so it need not be read again.
    pub(crate) fn edited(mut self, edits: &[(Range<usize>, &str)]) -> Self {
        let mut shift: isize = 0;
        for (range, replacement) in edits.iter().rev() {
            let (from, to) = (range.start - self.start, range.end - self.start);
            self.text.replace_range(from..to, replacement);
            shift += replacement.len().cast_signed() - range.len().cast_signed();
        }
        self.next = self.next.saturating_add_signed(shift);
        self
    }
}

/// The offset of the character after `at` in `line`, or one past its end.
fn char_after(line: &str, at: usize) -> usize {
    line[at..]
        .chars()
        .next()
        .map_or(at + 1, |c| at + c.len_utf8())
}

/// A search for a regexp's matches through ranges of a rope, match after match.
pub(crate) struct Scan<'a> {
    regexp: &'a Regexp,
    text: &'a Rope,
    ranges: &'a [Range<usize>],
    range: usize,
    at: usize,
    /// An empty match here is skipped: a match ended here.
    skip_empty_at: Option<usize>,
    line: Option<Line>,
    /// The work done, against `limit`.
    work: usize,
    limit: usize,
    /// The work at the last look at whether to stop.
    looked: usize,
    /// When it stops, whatever its work, for a search on the UI thread: a pattern can read
    /// far past each match it finds.
    until: Option<std::time::Instant>,
}

impl<'a> Scan<'a> {
    /// A search of `regexp` through `ranges` of `text`, ascending and apart, from `from`,
    /// skipping an empty match there if `after_match`, doing at most `limit` of work. `line`
    /// is the text of a line it may start in, if the caller kept it.
    pub(crate) fn new(
        regexp: &'a Regexp,
        text: &'a Rope,
        ranges: &'a [Range<usize>],
        (from, after_match): (usize, bool),
        limit: usize,
        line: Option<Line>,
    ) -> Self {
        Self {
            regexp,
            text,
            ranges,
            range: ranges.partition_point(|range| range.end < from),
            at: from,
            skip_empty_at: after_match.then_some(from),
            line,
            work: 0,
            limit,
            looked: 0,
            until: None,
        }
    }

    /// Stops at `until` too.
    pub(crate) fn until(&mut self, until: std::time::Instant) {
        self.until = Some(until);
    }

    /// Where it would go on, and whether a match ended there.
    pub(crate) fn place(&self) -> (usize, bool) {
        (self.at, self.skip_empty_at == Some(self.at))
    }

    /// The line it is in, for the caller to keep.
    pub(crate) fn into_line(self) -> Option<Line> {
        self.line
    }

    /// Adds `work`, and says whether the search must stop: past its limit, or as `stop`
    /// says, looked at every [`LOOK_EVERY`] of work.
    fn spend(&mut self, work: usize, stop: &dyn Fn() -> bool) -> bool {
        self.work = self.work.saturating_add(work);
        if self.work > self.limit {
            return true;
        }
        if self.work - self.looked > LOOK_EVERY {
            self.looked = self.work;
            return stop();
        }
        false
    }

    /// The next match: [`Found::Match`], [`Found::None`] past the last range, or
    /// [`Found::Stopped`] where it would go on, past its limit or as `stop` says.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] at a line longer than the regexp is matched in.
    pub(crate) fn next(&mut self, stop: &dyn Fn() -> bool) -> Result<Found, LineTooLong> {
        loop {
            let Some(range) = self.ranges.get(self.range).cloned() else {
                return Ok(Found::None);
            };
            let late = self
                .until
                .is_some_and(|until| std::time::Instant::now() >= until);
            if self.work > self.limit || late {
                return Ok(Found::Stopped { resume: self.at });
            }
            if self.at < range.start {
                self.at = range.start;
                self.skip_empty_at = None;
            }
            if self.at > range.end {
                self.range += 1;
                continue;
            }
            if !self.line.as_ref().is_some_and(|line| line.holds(self.at)) {
                let line = Line::read(self.text, self.at, self.regexp.longest)?;
                let cost = line.text.len() + PER_LINE;
                if self.spend(cost, stop) {
                    return Ok(Found::Stopped { resume: self.at });
                }
                self.line = Some(line);
            }
            let Some(line) = self.line.as_ref() else {
                continue;
            };
            let (start, next) = (line.start, line.next);
            let mut from = self.at - start;
            let found = loop {
                let found = self.regexp.regex.find_at(&line.text, from);
                // A look that finds nothing read to the line's end; one that finds a match read
                // about to its end, and a pattern that reads further is what `until` is for.
                let cost = found.map_or(line.text.len() - from, |found| found.end() - from);
                self.work = self.work.saturating_add(cost + PER_MATCH);
                let Some(found) = found else {
                    break None;
                };
                let matched = start + found.start()..start + found.end();
                if matched.is_empty() && self.skip_empty_at == Some(matched.start) {
                    from = char_after(&line.text, found.start());
                } else if matched.end > range.end && matched.start < range.end {
                    // The leftmost match runs past the range; one starting later may not.
                    from = char_after(&line.text, found.start());
                } else {
                    break Some(matched);
                }
                if from > line.text.len() {
                    break None;
                }
            };
            let over = self.work > self.limit
                || (self.work - self.looked > LOOK_EVERY && {
                    self.looked = self.work;
                    stop()
                });
            match found {
                Some(matched) if matched.end <= range.end => {
                    self.at = if !matched.is_empty() {
                        matched.end
                    } else if matched.start - start < line.text.len() {
                        start + char_after(&line.text, matched.start - start)
                    } else {
                        // Past an empty match at the line's end: the next line.
                        next
                    };
                    self.skip_empty_at = (!matched.is_empty()).then_some(matched.end);
                    return Ok(Found::Match(matched));
                }
                // Past the range's end: on to the next range.
                Some(_) => self.at = range.end + 1,
                None => {
                    self.at = next;
                    self.skip_empty_at = None;
                }
            }
            if over {
                return Ok(Found::Stopped { resume: self.at });
            }
        }
    }
}

impl Regexp {
    /// The replacement for the match at `range`: `to` with its `$1`, `${name}`, `$0` and `$$`
    /// expanded from the match's groups, as `Regex::replace` expands them. It reads the
    /// match and the character before it, not the whole line.
    pub(crate) fn expand(&self, text: &Rope, range: &Range<usize>, to: &str) -> String {
        let line_start = text.line_start(text.offset_to_point(range.start).row);
        // One character before it, for `\b`; at the line's start, `^` holds there as it did.
        let before = if range.start > line_start {
            text.clip_offset(range.start - 1, crate::sum_tree::Bias::Left)
        } else {
            range.start
        };
        let mut haystack = String::with_capacity(range.end - before);
        haystack.extend(text.chunks_in_range(before..range.end));
        let mut out = String::new();
        match self.regex.captures_at(&haystack, range.start - before) {
            Some(captures) => captures.expand(to, &mut out),
            None => out.push_str(to),
        }
        out
    }

    /// The matches within `spans`, the parts of lines a frame shows, at most `most`: each line
    /// at most `longest` bytes is read, and looked in from a little before its span to a little
    /// after, so a frame costs as much as its view, for the highlights. A match the window cuts
    /// at its end is left out, as it may be longer or none.
    pub(crate) fn matches_in(
        &self,
        text: &Rope,
        spans: &[Range<usize>],
        (most, longest): (usize, usize),
        out: &mut Vec<Range<usize>>,
    ) {
        let mut line = String::new();
        for span in spans {
            let row = text.offset_to_point(span.start).row;
            let (start, end) = (text.line_start(row), text.line_end(row));
            if end - start > longest || span.start > end {
                continue;
            }
            line.clear();
            line.extend(text.chunks_in_range(start..end));
            let floor = |at: usize| {
                let mut at = at.min(line.len());
                while !line.is_char_boundary(at) {
                    at -= 1;
                }
                at
            };
            let from = floor((span.start - start).saturating_sub(VIEW_REACH));
            let cut = floor((span.end.min(end) - start).saturating_add(VIEW_REACH));
            let haystack = &line[..cut];
            let mut at = from;
            let mut last_end = None;
            while at <= cut {
                let Some(found) = self.regex.find_at(haystack, at) else {
                    break;
                };
                let matched = start + found.start()..start + found.end();
                let cut_off = found.end() == cut && cut < line.len();
                let skipped = found.is_empty() && last_end == Some(found.start());
                at = if found.is_empty() {
                    char_after(haystack, found.start())
                } else {
                    found.end()
                };
                last_end = (!found.is_empty()).then_some(found.end());
                if matched.end <= span.start || matched.start >= span.end {
                    continue;
                }
                if !cut_off && !skipped && !matched.is_empty() {
                    out.push(matched);
                    if out.len() == most {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
impl Regexp {
    /// Matches in lines at most `longest` bytes long, for a test of the rule.
    pub(crate) fn set_longest_line(&mut self, longest: usize) {
        self.longest = longest;
    }
}

#[cfg(test)]
mod tests;
