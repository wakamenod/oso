//! The regexps of query-replace (ADR 0113) and isearch (ADR 0116): the `regex` crate's syntax,
//! matched one line at a time over a rope, with smart case, and a replacement's `$1`,
//! `${name}` and `$0`.
//!
//! Must not depend on: `buffer`, `editor` or a log. `Debug` prints no pattern, which can say
//! something of a buffer that may hold secrets.
//!
//! - A search keeps the line it is in and goes on in it match after match, so a line is read
//!   once however many matches it holds, and what it costs is counted: each line read, and
//!   each look for a match from where the last ended to the line's end. It asks whether to
//!   stop before each look, as one look can read to the end of a long line.
//! - For an isearch key on the UI thread ([`Limits::on_key`]), a line longer than the work left
//!   or than [`KEY_LINE`], and a regexp for a cut not built yet, are left to the worker.
//! - Empty matches go as `Regex::replace_all` takes them: one right where a match ended is
//!   skipped, and one at the end of a range counts.
//! - Backward, in [`back`], a search takes the match that starts last, as Emacs's
//!   `re-search-backward` does.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use regex::Regex;

use super::{Found, Limits};
use crate::rope::Rope;
use cut::{AtCut, Ending, Unbuilt};

mod back;
mod compile;
mod cut;

pub(crate) use back::find_backward;
use compile::builder;
pub(crate) use compile::{CompileError, On, compile, compile_isearch, compile_migemo, smart_case};

/// The longest line a regexp is matched in: past it, a search stops at the line.
pub(crate) const MAX_LINE: usize = 64 * 1024 * 1024;

/// The most characters of a regexp, as isearch's string (ADR 0082).
pub(crate) const MAX_PATTERN_CHARS: usize = 1024;

/// The most memory a compiled regexp takes: enough for `\w{30}` and `[\p{L}\d]{25}`.
const SIZE_LIMIT: usize = 2 * 1024 * 1024;

/// The most memory a regexp compiled on the UI thread takes, for an isearch key: one this
/// size compiles within about a millisecond, and a larger one compiles on the worker.
pub(crate) const KEY_SIZE_LIMIT: usize = 256 * 1024;

/// The most memory migemo's pattern with a dictionary's words takes (ADR 0116, Q7): past it,
/// the pattern drops the words and keeps the kana, within [`KEY_SIZE_LIMIT`].
pub(crate) const WORDS_SIZE_LIMIT: usize = 4 * 1024 * 1024;

/// What reading a line costs besides its bytes, as a line's seek and copy take time of
/// their own.
const PER_LINE: usize = 64;

/// The longest line an isearch key looks in on the UI thread: one look at a line this long
/// takes under a millisecond even where the crate's fastest engine gives up, as on a `\b`
/// beside text that is not ASCII.
const KEY_LINE: usize = 16 * 1024;

/// What a match found costs besides the text it spans.
const PER_MATCH: usize = 16;

/// How far past the view a frame's highlights look on a line.
const VIEW_REACH: usize = 256;

/// The most time a frame's highlights look for a regexp's matches: what is left of a frame
/// beside a key's search.
pub(crate) const FRAME_TIME: std::time::Duration = std::time::Duration::from_millis(2);

/// How many matches a frame's or a key's look finds between looks at the time.
const TIME_EVERY: usize = 256;

/// A line too long to match a regexp in, which starts at this offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineTooLong(pub(crate) usize);

/// A regexp compiled with smart case.
#[derive(Clone)]
pub(crate) struct Regexp {
    regex: Regex,
    /// For a match at a range's end inside a line, built when one is first needed, never on
    /// an isearch key; `None` if it would not compile. A clone made before it is built builds
    /// its own, and no two threads share one regexp, so none waits on another's.
    #[expect(
        clippy::disallowed_types,
        reason = "a lazy part of one value, not global state (ADR 0038)"
    )]
    at_cut: std::sync::OnceLock<Option<AtCut>>,
    /// The pattern and whether it folds case, for `at_cut`.
    source: Arc<(String, bool)>,
    /// A key compiled it quickly, so a key may build the regexps for a cut, which take about
    /// twice as long.
    quick: bool,
    /// It has no assertion, as migemo's pattern has none, so a match that ends at a cut is one
    /// as it is, and it needs no regexps for a cut.
    plain: bool,
    /// The longest line it is matched in.
    longest: usize,
}

impl fmt::Debug for Regexp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Regexp").finish_non_exhaustive()
    }
}

/// Two regexps are the same when they are compiled from the same pattern, the same way.
impl PartialEq for Regexp {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.longest == other.longest
    }
}

impl Eq for Regexp {}

/// What a search may do and what it has done: its work as counted, against its limit, and
/// when it stops.
pub(super) struct Effort {
    pub(super) work: usize,
    limit: usize,
    until: Option<Instant>,
    on_key: bool,
}

impl Effort {
    pub(super) fn new(limits: Limits) -> Self {
        Self {
            work: 0,
            limit: limits.work,
            until: limits.until,
            on_key: limits.on_key,
        }
    }

    /// Whether to stop: past the limit or `until`, or as `stop` says, asked before each look,
    /// as one look can read a whole line.
    pub(super) fn over(&self, stop: &dyn Fn() -> bool) -> bool {
        self.work > self.limit || self.until.is_some_and(|until| Instant::now() >= until) || stop()
    }

    /// Whether the regexps for a cut of `regexp` may be built here: on an isearch key, only
    /// for one it compiled quickly.
    pub(super) fn may_build(&self, regexp: &Regexp) -> bool {
        !self.on_key || regexp.quick
    }

    /// Whether a line of `len` bytes is left to the worker rather than read here.
    fn leaves(&self, len: usize) -> bool {
        self.on_key && (len > KEY_LINE || len + PER_LINE > self.limit.saturating_sub(self.work))
    }
}

/// `at` in `text`, or its end, stepped back to a character's start.
pub(super) fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
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

/// The row `offset` is on, and where it starts and ends, without its line break.
pub(super) fn span(text: &Rope, offset: usize) -> (usize, usize, usize) {
    let row = text.offset_to_point(offset).row;
    (row, text.line_start(row), text.line_end(row))
}

impl Line {
    /// Reads the line `offset` is on into `slot`, in the room of the line it held, with its
    /// cost added to `effort`. Returns false, and leaves `slot` as it was, if `effort` leaves
    /// a line that long to the worker.
    pub(super) fn read(
        text: &Rope,
        offset: usize,
        (longest, effort): (usize, &mut Effort),
        slot: &mut Option<Self>,
    ) -> Result<bool, LineTooLong> {
        let (row, start, end) = span(text, offset);
        if end - start > longest {
            return Err(LineTooLong(start));
        }
        if effort.leaves(end - start) {
            return Ok(false);
        }
        let next = if row >= text.summary().lines {
            text.len() + 1
        } else {
            text.line_start(row + 1)
        };
        let mut line = slot.take().map(|line| line.text).unwrap_or_default();
        line.clear();
        line.extend(text.chunks_in_range(start..end));
        effort.work = effort.work.saturating_add(end - start + PER_LINE);
        *slot = Some(Self {
            start,
            next,
            text: line,
        });
        Ok(true)
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
pub(super) fn char_after(line: &str, at: usize) -> usize {
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
    /// Where a match that ends at a range's end inside its line starts, kept.
    ending: Ending,
    effort: Effort,
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
            ending: Ending::default(),
            effort: Effort::new(Limits {
                work: limit,
                ..Limits::NONE
            }),
        }
    }

    /// Stops at `until` too.
    pub(crate) fn until(&mut self, until: Instant) {
        self.effort.until = Some(until);
    }

    /// Searches for an isearch key on the UI thread: it leaves a long line, and a regexp for
    /// a cut not built yet, to the worker.
    pub(crate) fn on_key(&mut self) {
        self.effort.on_key = true;
    }

    /// Where it would go on, and whether a match ended there.
    pub(crate) fn place(&self) -> (usize, bool) {
        (self.at, self.skip_empty_at == Some(self.at))
    }

    /// The line it is in, for the caller to keep.
    pub(crate) fn into_line(self) -> Option<Line> {
        self.line
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
            if self.effort.over(stop) {
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
                let limits = (self.regexp.longest, &mut self.effort);
                if !Line::read(self.text, self.at, limits, &mut self.line)? {
                    return Ok(Found::Stopped { resume: self.at });
                }
                if self.effort.over(stop) {
                    return Ok(Found::Stopped { resume: self.at });
                }
            }
            let Some(line) = self.line.as_ref() else {
                continue;
            };
            let (start, next) = (line.start, line.next);
            let mut from = self.at - start;
            // The range is the haystack, as in Emacs's bounded search: a match ends within it,
            // so a greedy pattern stops at its end, while `$` and `\b` there see what follows.
            let cut = floor_boundary(&line.text, range.end - start);
            let skip_empty_at = self.skip_empty_at;
            let found = loop {
                let looked = cut::find(
                    self.regexp,
                    (&line.text, start, cut),
                    from,
                    &mut self.ending,
                    &mut self.effort,
                );
                let Some(found) = (match looked {
                    Ok(found) => found,
                    Err(Unbuilt) => {
                        self.skip_empty_at = skip_empty_at;
                        return Ok(Found::Stopped { resume: self.at });
                    }
                }) else {
                    break None;
                };
                let matched = start + found.start..start + found.end;
                if !(matched.is_empty() && self.skip_empty_at == Some(matched.start)) {
                    break Some(matched);
                }
                // An empty match right where a match ended is skipped, once.
                self.skip_empty_at = None;
                from = char_after(&line.text[..cut], found.start);
            };
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
                // Nothing more in the range's part of this line: on past the range if it ends
                // in the line, as another range may start further along it, or to the next.
                None if cut < line.text.len() => {
                    self.at = range.end + 1;
                    self.skip_empty_at = None;
                }
                None => {
                    self.at = next;
                    self.skip_empty_at = None;
                }
            }
        }
    }
}

/// Room for a line's text that a caller keeps from call to call, so a frame or a key allocates
/// nothing once it has grown. `Debug` prints none of the text (ADR 0044).
#[derive(Default)]
pub(crate) struct LineRoom(String);

impl fmt::Debug for LineRoom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LineRoom")
    }
}

impl Regexp {
    /// The bytes of its pattern, for the measures of migemo's needles.
    #[cfg(test)]
    pub(crate) fn pattern_len(&self) -> usize {
        self.source.0.len()
    }

    /// Its first match in `text`, for tests of what a pattern matches.
    #[cfg(test)]
    pub(crate) fn find_in(&self, text: &str) -> Option<String> {
        self.regex.find(text).map(|found| found.as_str().to_owned())
    }

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

    /// The matches within `spans`, the parts of lines a frame shows, at most `most`, for the
    /// highlights: each line is read and looked in from a little before its span to a little
    /// after, from one character before that, so `^` and `\b` see what is there, reading at
    /// most `limit` bytes in all and until `until` if one is given, so a frame or a key costs
    /// as much as its view. A match the window cuts at its end is left out, as it may be longer or none.
    /// `room` is where each line's text is read.
    pub(crate) fn matches_in(
        &self,
        text: &Rope,
        spans: &[Range<usize>],
        (most, limit): (usize, usize),
        (until, room): (Option<Instant>, &mut LineRoom),
        out: &mut Vec<Range<usize>>,
    ) {
        let passed = || until.is_some_and(|until| Instant::now() >= until);
        let (mut left, line) = (limit, &mut room.0);
        let mut looks = 0_usize;
        for shown in spans {
            let (_, start, end) = span(text, shown.start);
            if shown.start > end || passed() {
                continue;
            }
            let clip = |at: usize| text.clip_offset(at, crate::sum_tree::Bias::Left);
            let look = clip(shown.start.saturating_sub(VIEW_REACH).max(start));
            let read_from = if look > start { clip(look - 1) } else { start };
            let read_to = clip(shown.end.min(end).saturating_add(VIEW_REACH).min(end));
            let Some(rest) = left.checked_sub(read_to - read_from) else {
                return;
            };
            left = rest;
            line.clear();
            line.extend(text.chunks_in_range(read_from..read_to));
            let (cut, at_end) = (line.len(), read_to == end);
            let mut at = look - read_from;
            let mut last_end = None;
            while at <= cut {
                // A line full of matches looks at the time now and then, too.
                looks += 1;
                if looks.is_multiple_of(TIME_EVERY) && passed() {
                    return;
                }
                let Some(found) = self.regex.find_at(line, at) else {
                    break;
                };
                let matched = read_from + found.start()..read_from + found.end();
                let cut_off = found.end() == cut && !at_end;
                let skipped = found.is_empty() && last_end == Some(found.start());
                at = if found.is_empty() {
                    char_after(line, found.start())
                } else {
                    found.end()
                };
                last_end = (!found.is_empty()).then_some(found.end());
                if matched.end <= shown.start || matched.start >= shown.end {
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

/// `text` escaped for a regexp, as Emacs's `regexp-quote` does, cut to at most `room`
/// characters, never inside an escape.
pub(crate) fn quote(text: &str, room: usize) -> String {
    let (mut out, mut left) = (String::new(), room);
    let mut one = [0; 4];
    for c in text.chars() {
        let escaped = regex::escape(c.encode_utf8(&mut one));
        let Some(rest) = left.checked_sub(escaped.chars().count()) else {
            break;
        };
        left = rest;
        out.push_str(&escaped);
    }
    out
}

#[cfg(test)]
impl Regexp {
    /// Matches in lines at most `longest` bytes long, for a test of the rule.
    pub(crate) fn set_longest_line(&mut self, longest: usize) {
        self.longest = longest;
    }

    /// Whether the regexps for a cut are built, for a test that they wait.
    pub(crate) fn cut_built(&self) -> bool {
        self.at_cut.get().is_some()
    }
}

#[cfg(test)]
mod isearch_tests;
#[cfg(test)]
mod tests;
