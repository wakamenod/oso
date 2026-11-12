//! Search over a rope, for isearch (ADR 0082) and query-replace (ADR 0113): a literal query
//! built from a string with smart case, searched forward, backward, in a range and through
//! the whole text, and the regexps of `regexp`, matched one line at a time.
//!
//! Must not depend on: `buffer`, `motion`, `editor`, threads or a log. The worker's
//! [`SearchJob`] is a plain function here, and the host runs it on its thread.
//!
//! - The text is read one character at a time with Knuth–Morris–Pratt, so the cost is
//!   linear in the text whatever the string, and a match may cross chunks.
//! - Case folds one character at a time: a character whose lowercase is one character
//!   becomes it, and any other stays. Width and characters are not folded.
//! - The U+FFFD of an invalid byte matches nothing in a literal query (ADR 0040); a regexp's
//!   `.` matches it, as in query-replace.
//! - Nothing here logs, and `Debug` prints no text of a query or a job: a search string can
//!   say something of a buffer that may hold secrets (ADR 0044).

use std::fmt;
use std::ops::Range;
use std::time::{Duration, Instant};

use crate::decode::InvalidBytes;
use crate::rope::Rope;
use crate::sum_tree::Bias;

mod case;
mod job;
mod needle;
mod regexp;

pub(crate) use case::casified;
pub(crate) use job::{Counting, Find, MatchWork, Matched, Prepared, Report, after};
pub use job::{SearchJob, SearchReport};
pub(crate) use needle::Needle;
pub(crate) use regexp::{
    CompileError, Line, LineTooLong, MAX_LINE, On, Regexp, compile as compile_regexp,
    compile_isearch, quote, smart_case as regexp_smart_case,
};

/// The most characters a search string holds. What is added past it is cut, so that what a
/// frame looks at around the view stays small.
pub const MAX_QUERY_CHARS: usize = 1024;

/// How much text one key searches on the UI thread before the rest goes to a worker.
pub const KEY_BUDGET: usize = 256 * 1024;

/// The most time a key's search takes on the UI thread, as a regexp can read far past each
/// place it looks.
pub(crate) const KEY_TIME: Duration = Duration::from_millis(3);

/// How far a search goes before it stops: its work, which for a literal query is the bytes it
/// reads, a time, and whether it is an isearch key's on the UI thread, which leaves a regexp's
/// long lines and the regexps for a cut to the worker.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) work: usize,
    pub(crate) until: Option<Instant>,
    pub(crate) on_key: bool,
}

impl Limits {
    /// No bound but `stop`, for the worker.
    pub(crate) const NONE: Self = Self {
        work: usize::MAX,
        until: None,
        on_key: false,
    };

    /// A key's on the UI thread: [`KEY_BUDGET`], [`KEY_TIME`] from now, and the rest left to
    /// the worker.
    pub(crate) fn key() -> Self {
        Self {
            work: KEY_BUDGET,
            until: Some(Instant::now() + KEY_TIME),
            on_key: true,
        }
    }
}

/// The bytes a backward search reads at once, and a worker reads between looks at whether a
/// newer job has taken its place.
const BLOCK: usize = 64 * 1024;

/// What the U+FFFD of an invalid byte reads as: no character, so nothing matches it.
const INVALID: u32 = u32::MAX;

/// Whether `string` makes the search case-sensitive: it holds an uppercase letter.
pub fn smart_case(string: &str) -> bool {
    string.chars().any(char::is_uppercase)
}

/// `c` folded to its lowercase, if that is one character. Kana, CJK and Hangul have no
/// case, so they skip the tables.
fn fold(c: char) -> u32 {
    let code = u32::from(c);
    if c.is_ascii() {
        return u32::from(c.to_ascii_lowercase());
    }
    if matches!(code, 0x3040..=0x9FFF | 0xAC00..=0xD7A3) {
        return code;
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(lower), None) => u32::from(lower),
        _ => code,
    }
}

/// The most bytes a character of the text can take where the pattern has `key`: the
/// character's own, but for a folded one, which any of several can fold to. Only the Kelvin
/// sign folds to an ASCII letter (`k`), and a folded character outside ASCII is taken at
/// four.
fn reach(key: u32, case_sensitive: bool) -> usize {
    let Some(c) = char::from_u32(key) else {
        return 4;
    };
    match (case_sensitive, c) {
        (true, c) => c.len_utf8(),
        (false, 'k') => '\u{212A}'.len_utf8(),
        (false, c) if c.is_ascii() => 1,
        (false, _) => 4,
    }
}

/// A string to look for, with the table Knuth–Morris–Pratt steps back by.
#[derive(Clone)]
struct Pattern {
    chars: Box<[u32]>,
    /// For each length matched, the longest proper prefix that is also a suffix.
    fail: Box<[usize]>,
}

impl Pattern {
    fn new(chars: Vec<u32>) -> Self {
        let mut fail = vec![0; chars.len()];
        let mut k = 0;
        for i in 1..chars.len() {
            while k > 0 && chars[i] != chars[k] {
                k = fail[k - 1];
            }
            if chars[i] == chars[k] {
                k += 1;
            }
            fail[i] = k;
        }
        Self {
            chars: chars.into(),
            fail: fail.into(),
        }
    }
}

/// What a search looks for: a string, cut to [`MAX_QUERY_CHARS`], and whether case matters.
#[derive(Clone)]
pub struct Query {
    forward: Pattern,
    /// The same characters, last first, for searching backward.
    backward: Pattern,
    case_sensitive: bool,
    max_bytes: usize,
}

impl fmt::Debug for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The characters stay out of it (ADR 0044).
        f.debug_struct("Query")
            .field("chars", &self.forward.chars.len())
            .field("case_sensitive", &self.case_sensitive)
            .finish_non_exhaustive()
    }
}

impl Query {
    /// A query for `string`, `None` if it is empty.
    pub fn new(string: &str, case_sensitive: bool) -> Option<Self> {
        let key = |c: char| {
            if case_sensitive {
                u32::from(c)
            } else {
                fold(c)
            }
        };
        let chars: Vec<u32> = string.chars().take(MAX_QUERY_CHARS).map(key).collect();
        if chars.is_empty() {
            return None;
        }
        let max_bytes = chars.iter().map(|&key| reach(key, case_sensitive)).sum();
        let mut reversed = chars.clone();
        reversed.reverse();
        Some(Self {
            forward: Pattern::new(chars),
            backward: Pattern::new(reversed),
            case_sensitive,
            max_bytes,
        })
    }

    /// The most bytes a match can take, as a folded character can stand for a longer one
    /// (`k` for the Kelvin sign).
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// What the character `c` of the text reads as, `invalid` if it is the U+FFFD of an
    /// invalid byte.
    fn key(&self, c: char, invalid: bool) -> u32 {
        if invalid {
            INVALID
        } else if self.case_sensitive {
            u32::from(c)
        } else {
            fold(c)
        }
    }
}

/// Knuth–Morris–Pratt over one pattern, fed a character at a time, remembering where the
/// last characters were, so that a match says where it began.
struct Matcher<'p> {
    pattern: &'p Pattern,
    /// A match may begin inside the one before, so that none is passed over, as a search
    /// that takes only some of them needs. Otherwise matches do not overlap.
    overlapping: bool,
    matched: usize,
    /// The other end of each of the last characters fed, as a ring.
    ends: Vec<usize>,
    /// Where the next goes in the ring.
    next: usize,
}

impl<'p> Matcher<'p> {
    fn new(pattern: &'p Pattern) -> Self {
        Self {
            pattern,
            overlapping: false,
            matched: 0,
            ends: vec![0; pattern.chars.len()],
            next: 0,
        }
    }

    /// A matcher that finds every match, also those that begin inside another.
    fn overlapping(pattern: &'p Pattern) -> Self {
        Self {
            overlapping: true,
            ..Self::new(pattern)
        }
    }

    /// Forgets a match under way, for text that does not follow on.
    fn reset(&mut self) {
        self.matched = 0;
    }

    /// Feeds the character `key` with `other_end`, the end it would give a match it starts.
    /// Returns that end of the match it completes, if it does.
    fn feed(&mut self, key: u32, other_end: usize) -> Option<usize> {
        let chars = &self.pattern.chars;
        let len = chars.len();
        self.ends[self.next] = other_end;
        self.next += 1;
        if self.next == len {
            self.next = 0;
        }
        while self.matched > 0 && chars[self.matched] != key {
            self.matched = self.pattern.fail[self.matched - 1];
        }
        if chars[self.matched] == key {
            self.matched += 1;
        }
        if self.matched < len {
            return None;
        }
        self.matched = if self.overlapping {
            self.pattern.fail[len - 1]
        } else {
            0
        };
        // The ring is full, so the oldest is where the next goes.
        Some(self.ends[self.next])
    }
}

/// What a search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// A match, as a range of the text.
    Match(Range<usize>),
    /// There is none in the text it looked through.
    None,
    /// It stopped before the end of what it was to look through: at its budget, or for a
    /// newer job. The search goes on from `resume`, which a match it was in the middle of
    /// is not past.
    Stopped { resume: usize },
}

/// Feeds the characters of `range` to `on_char`, first to last, with their offset and
/// whether each is the U+FFFD of an invalid byte, until it returns false. Stops once
/// `limit` bytes are read, or `stop` says so between blocks, and returns the offset it had
/// reached then. The invalid bytes are walked alongside, so each costs O(1).
fn walk_forward(
    (text, invalid): (&Rope, &InvalidBytes),
    range: Range<usize>,
    limit: usize,
    stop: &dyn Fn() -> bool,
    mut on_char: impl FnMut(char, usize, bool) -> bool,
) -> Option<usize> {
    let mut offset = text.clip_offset(range.start, Bias::Left);
    let mut bad = invalid
        .range(offset..range.end)
        .map(|(at, _)| at)
        .peekable();
    let (mut read, mut unchecked) = (0usize, 0usize);
    for chunk in text.chunks_in_range(offset..range.end) {
        if read >= limit {
            return Some(offset);
        }
        if unchecked >= BLOCK {
            unchecked = 0;
            if stop() {
                return Some(offset);
            }
        }
        for (index, c) in chunk.char_indices() {
            let at = offset + index;
            let is_bad = c == char::REPLACEMENT_CHARACTER && {
                while bad.next_if(|&next| next < at).is_some() {}
                bad.peek() == Some(&at)
            };
            if !on_char(c, at, is_bad) {
                return None;
            }
        }
        offset += chunk.len();
        read += chunk.len();
        unchecked += chunk.len();
    }
    None
}

/// Feeds the characters of `range` to `on_char` as [`walk_forward`] does, but last to first,
/// and returns where it stopped as the lowest offset read.
fn walk_backward(
    (text, invalid): (&Rope, &InvalidBytes),
    range: Range<usize>,
    limit: usize,
    stop: &dyn Fn() -> bool,
    mut on_char: impl FnMut(char, usize, bool) -> bool,
) -> Option<usize> {
    let start = text.clip_offset(range.start, Bias::Left);
    let mut high = text.clip_offset(range.end, Bias::Left).max(start);
    let mut read = 0usize;
    let mut chunks: Vec<&str> = Vec::new();
    let mut bad: Vec<usize> = Vec::new();
    while high > start {
        if read >= limit || (read > 0 && stop()) {
            return Some(high);
        }
        let low = text.clip_offset(high.saturating_sub(BLOCK).max(start), Bias::Left);
        chunks.clear();
        chunks.extend(text.chunks_in_range(low..high));
        bad.clear();
        bad.extend(invalid.range(low..high).map(|(at, _)| at));
        let mut end = high;
        for chunk in chunks.iter().rev() {
            let base = end - chunk.len();
            for (index, c) in chunk.char_indices().rev() {
                let at = base + index;
                let is_bad = c == char::REPLACEMENT_CHARACTER && {
                    while bad.last().is_some_and(|&last| last > at) {
                        bad.pop();
                    }
                    bad.last() == Some(&at)
                };
                if !on_char(c, at, is_bad) {
                    return None;
                }
            }
            end = base;
        }
        read += high - low;
        high = low;
    }
    None
}

/// The first match of `query` that starts at `from` or after, reading at most about `limit`
/// bytes, and none past `stop`.
pub fn find_forward(
    text: &Rope,
    invalid: &InvalidBytes,
    query: &Query,
    from: usize,
    limit: usize,
    stop: &dyn Fn() -> bool,
) -> Found {
    find_in(text, invalid, query, from..text.len(), limit, stop)
}

/// The first match of `query` within `range`, reading at most about `limit` bytes, and none
/// past `stop`. A match that would end past the range is not one.
pub(crate) fn find_in(
    text: &Rope,
    invalid: &InvalidBytes,
    query: &Query,
    range: Range<usize>,
    limit: usize,
    stop: &dyn Fn() -> bool,
) -> Found {
    let from = range.start;
    let mut matcher = Matcher::new(&query.forward);
    let mut found = None;
    let stopped = walk_forward(
        (text, invalid),
        range,
        limit,
        stop,
        |c, at, bad| match matcher.feed(query.key(c, bad), at) {
            Some(start) => {
                found = Some(start..at + c.len_utf8());
                false
            }
            None => true,
        },
    );
    match (found, stopped) {
        (Some(range), _) => Found::Match(range),
        (None, Some(at)) => {
            // A match under way began at most this far back.
            let back = at.saturating_sub(query.max_bytes()).max(from);
            Found::Stopped {
                resume: text.clip_offset(back, Bias::Left),
            }
        }
        (None, None) => Found::None,
    }
}

/// The last match of `query` that ends at `before` or earlier and starts at `start_at_most`
/// or earlier, reading at most about `limit` bytes, and none past `stop`. A match that
/// starts where an earlier one ends is found from its end, so with `start_at_most` at
/// `before` this is the last match before `before`.
pub fn find_backward(
    text: &Rope,
    invalid: &InvalidBytes,
    query: &Query,
    before: usize,
    start_at_most: usize,
    limit: usize,
    stop: &dyn Fn() -> bool,
) -> Found {
    // One that ends too late to take must not hide one inside it that would do.
    let mut matcher = Matcher::overlapping(&query.backward);
    let mut found = None;
    let stopped = walk_backward(
        (text, invalid),
        0..before,
        limit,
        stop,
        |c, at, bad| match matcher.feed(query.key(c, bad), at + c.len_utf8()) {
            Some(end) if at <= start_at_most => {
                found = Some(at..end);
                false
            }
            _ => true,
        },
    );
    match (found, stopped) {
        (Some(range), _) => Found::Match(range),
        (None, Some(at)) => {
            let ahead = at.saturating_add(query.max_bytes()).min(before);
            Found::Stopped {
                resume: text.clip_offset(ahead, Bias::Left),
            }
        }
        (None, None) => Found::None,
    }
}

/// The matches of `query` in each of `spans`, which are in order and apart, into `out`, in
/// order and not overlapping, as a scan from each span's start finds them: at most `most`
/// of them, reading at most about `limit` bytes in all.
pub fn matches_in(
    (text, invalid): (&Rope, &InvalidBytes),
    query: &Query,
    spans: &[Range<usize>],
    (most, limit): (usize, usize),
    out: &mut Vec<Range<usize>>,
) {
    let mut matcher = Matcher::new(&query.forward);
    let mut left = limit;
    for span in spans {
        if out.len() >= most || left == 0 {
            return;
        }
        matcher.reset();
        let len = span.end.saturating_sub(span.start);
        let stopped = walk_forward(
            (text, invalid),
            span.clone(),
            left,
            &|| false,
            |c, at, bad| {
                if let Some(start) = matcher.feed(query.key(c, bad), at) {
                    out.push(start..at + c.len_utf8());
                    return out.len() < most;
                }
                true
            },
        );
        left = match stopped {
            Some(_) => 0,
            None => left.saturating_sub(len),
        };
    }
}

/// How many matches a text holds, and how many of them start before the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Count {
    pub total: usize,
    pub before: usize,
}

/// Counts every match of `query` in `text`, as a scan from the start finds them, and those
/// that start before `current`. `None` if `stop` said so first.
pub fn count(
    text: &Rope,
    invalid: &InvalidBytes,
    query: &Query,
    current: Option<usize>,
    stop: &dyn Fn() -> bool,
) -> Option<Count> {
    let mut matcher = Matcher::new(&query.forward);
    let mut count = Count {
        total: 0,
        before: 0,
    };
    let range = 0..text.len();
    let stopped = walk_forward((text, invalid), range, usize::MAX, stop, |c, at, bad| {
        if let Some(start) = matcher.feed(query.key(c, bad), at) {
            count.total += 1;
            if current.is_some_and(|current| start < current) {
                count.before += 1;
            }
        }
        true
    });
    stopped.is_none().then_some(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    fn never() -> bool {
        false
    }

    fn query(string: &str) -> Query {
        Query::new(string, smart_case(string)).unwrap()
    }

    fn forward(text: &str, string: &str, from: usize) -> Found {
        let rope = Rope::from(text);
        find_forward(
            &rope,
            &InvalidBytes::new(),
            &query(string),
            from,
            usize::MAX,
            &never,
        )
    }

    fn backward(text: &str, string: &str, before: usize) -> Found {
        let rope = Rope::from(text);
        let q = query(string);
        find_backward(
            &rope,
            &InvalidBytes::new(),
            &q,
            before,
            before,
            usize::MAX,
            &never,
        )
    }

    fn all(text: &str, string: &str) -> Vec<Range<usize>> {
        let rope = Rope::from(text);
        let mut out = Vec::new();
        let whole = 0..rope.len();
        let limits = (usize::MAX, usize::MAX);
        matches_in(
            (&rope, &InvalidBytes::new()),
            &query(string),
            std::slice::from_ref(&whole),
            limits,
            &mut out,
        );
        out
    }

    #[test]
    fn smart_case_folds_until_an_uppercase_letter() {
        assert_eq!(forward("say Foo", "foo", 0), Found::Match(4..7));
        assert_eq!(forward("say foo", "Foo", 0), Found::None);
        assert_eq!(forward("say Foo", "Foo", 0), Found::Match(4..7));
        assert!(!smart_case("日本語 123"));
        assert!(smart_case("Ä"));
        assert_eq!(forward("ÄRGER", "ärger", 0), Found::Match(0..6));
    }

    #[test]
    fn width_is_not_folded() {
        assert_eq!(forward("Ａ", "a", 0), Found::None);
        assert_eq!(forward("ａ", "Ａ", 0), Found::None);
        assert_eq!(forward("Ａ", "ａ", 0), Found::Match(0..3));
    }

    #[test]
    fn a_folded_character_can_be_longer_than_the_one_typed() {
        // The Kelvin sign lowercases to `k`.
        assert_eq!(forward("1 \u{212A}m", "km", 0), Found::Match(2..6));
    }

    #[test]
    fn forward_finds_the_first_that_starts_there_or_after() {
        assert_eq!(forward("ab ab ab", "ab", 0), Found::Match(0..2));
        assert_eq!(forward("ab ab ab", "ab", 1), Found::Match(3..5));
        assert_eq!(forward("ab ab ab", "ab", 7), Found::None);
    }

    #[test]
    fn backward_finds_the_last_that_ends_there_or_before() {
        assert_eq!(backward("ab ab ab", "ab", 8), Found::Match(6..8));
        assert_eq!(backward("ab ab ab", "ab", 7), Found::Match(3..5));
        assert_eq!(backward("ab ab ab", "ab", 1), Found::None);
    }

    #[test]
    fn backward_with_a_start_limit_keeps_a_match_that_grew() {
        let rope = Rope::from("x foo foo");
        let q = query("foo");
        // Typing `o` at a match of `fo` at 6 keeps it there.
        let found = find_backward(&rope, &InvalidBytes::new(), &q, 9, 6, usize::MAX, &never);
        assert_eq!(found, Found::Match(6..9));
        let found = find_backward(&rope, &InvalidBytes::new(), &q, 9, 5, usize::MAX, &never);
        assert_eq!(found, Found::Match(2..5));
    }

    #[test]
    fn matches_cross_chunks() {
        let text = format!("{}needle{}", "x".repeat(29), "y".repeat(40));
        assert_eq!(forward(&text, "needle", 0), Found::Match(29..35));
        assert_eq!(backward(&text, "needle", text.len()), Found::Match(29..35));
    }

    #[test]
    fn kmp_steps_back_inside_a_partial_match() {
        assert_eq!(forward("aaab", "aab", 0), Found::Match(1..4));
        assert_eq!(forward("abababc", "ababc", 0), Found::Match(2..7));
        assert_eq!(backward("abababc", "ababc", 7), Found::Match(2..7));
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(all("aaaa", "aa"), vec![0..2, 2..4]);
        let rope = Rope::from("aaaa");
        let found = count(&rope, &InvalidBytes::new(), &query("aa"), Some(2), &never);
        assert_eq!(
            found,
            Some(Count {
                total: 2,
                before: 1
            })
        );
    }

    #[test]
    fn an_invalid_byte_matches_nothing() {
        let snapshot = crate::load::decode_all(
            b"a\xffb a\xEF\xBF\xBDb",
            crate::display_map::TabWidth::default(),
        );
        let (rope, invalid) = (snapshot.text(), snapshot.display.invalid_bytes());
        let q = query("a\u{FFFD}b");
        let found = find_forward(rope, invalid, &q, 0, usize::MAX, &never);
        // The first is an invalid byte, the second a U+FFFD in the file.
        assert_eq!(found, Found::Match(6..11));
        assert_eq!(
            count(rope, invalid, &q, None, &never).map(|c| c.total),
            Some(1)
        );
    }

    #[test]
    fn a_budget_stops_the_search_where_it_can_go_on() {
        let text = format!("{}needle", "x".repeat(100));
        let rope = Rope::from(text.as_str());
        let q = query("needle");
        let found = find_forward(&rope, &InvalidBytes::new(), &q, 0, 40, &never);
        let Found::Stopped { resume } = found else {
            panic!("{found:?}");
        };
        assert!(resume < 100);
        let rest = find_forward(&rope, &InvalidBytes::new(), &q, resume, usize::MAX, &never);
        assert_eq!(rest, Found::Match(100..106));
        let found = find_backward(&rope, &InvalidBytes::new(), &q, 30, 30, 0, &never);
        assert_eq!(found, Found::Stopped { resume: 30 });
    }

    #[test]
    fn a_job_reports_its_find_then_its_count() {
        let rope = Rope::from("a b a b a");
        let live = Arc::new(AtomicU64::new(3));
        let job = SearchJob::new(
            (rope, InvalidBytes::new()),
            Prepared::Ready(Needle::Literal(query("a"))),
            Some(Find::Forward {
                from: 1,
                after_match: false,
            }),
            None,
            (3, live),
        );
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        assert_eq!(
            reports,
            [
                SearchReport(Report::Found {
                    generation: 3,
                    at: Some(4..5)
                }),
                SearchReport(Report::Counted {
                    generation: 3,
                    count: Count {
                        total: 3,
                        before: 1
                    },
                    at: Some(4)
                }),
            ]
        );
    }

    #[test]
    fn a_superseded_job_reports_nothing() {
        let live = Arc::new(AtomicU64::new(4));
        let job = SearchJob::new(
            (
                Rope::from("a".repeat(200_000).as_str()),
                InvalidBytes::new(),
            ),
            Prepared::Ready(Needle::Literal(query("a"))),
            None,
            None,
            (3, live),
        );
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        assert!(reports.is_empty());
    }

    #[test]
    fn a_reach_holds_every_character_that_folds_to_it() {
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            for sensitive in [false, true] {
                let key = if sensitive { u32::from(c) } else { fold(c) };
                assert!(c.len_utf8() <= reach(key, sensitive), "{:X}", u32::from(c));
            }
        }
    }

    #[test]
    fn invalid_bytes_match_nothing_backward_and_in_spans() {
        let snapshot = crate::load::decode_all(
            b"\xffb a\xEF\xBF\xBDb \xff\xffb",
            crate::display_map::TabWidth::default(),
        );
        let (rope, invalid) = (snapshot.text(), snapshot.display.invalid_bytes());
        let q = query("\u{FFFD}b");
        let found = find_backward(
            rope,
            invalid,
            &q,
            rope.len(),
            rope.len(),
            usize::MAX,
            &never,
        );
        assert_eq!(found, Found::Match(6..10));
        let mut out = Vec::new();
        let spans = [0..5, 5..rope.len()];
        matches_in(
            (rope, invalid),
            &q,
            &spans,
            (usize::MAX, usize::MAX),
            &mut out,
        );
        assert_eq!((out.len(), out.first()), (1, Some(&(6..10))));
    }

    #[test]
    fn backward_with_a_start_limit_finds_a_match_inside_one_it_passes() {
        for (text, string, before, start_at_most, expected) in [
            // `é` is two bytes: the match at 6 grows to two characters and stays there.
            ("éééééé", "éé", 12, 6, 6..10),
            ("kkkk", "kk", 4, 1, 1..3),
            ("日日日日", "日日日", 12, 3, 3..12),
        ] {
            let rope = Rope::from(text);
            let q = query(string);
            let found = find_backward(
                &rope,
                &InvalidBytes::new(),
                &q,
                before,
                start_at_most,
                usize::MAX,
                &never,
            );
            assert_eq!(found, Found::Match(expected), "{text}");
        }
    }

    #[test]
    fn debug_shows_no_text() {
        let q = query("hunter2");
        assert!(!format!("{q:?}").contains("hunter"));
    }

    #[test]
    fn a_query_is_cut_to_its_most_characters() {
        let long = "a".repeat(MAX_QUERY_CHARS + 10);
        assert_eq!(query(&long).max_bytes(), MAX_QUERY_CHARS);
        assert!(Query::new("", false).is_none());
    }

    /// The matches a naive scan finds, not overlapping, folded as the query folds.
    fn naive(text: &str, string: &str) -> Vec<Range<usize>> {
        let sensitive = smart_case(string);
        let key = |c: char| if sensitive { u32::from(c) } else { fold(c) };
        let needle: Vec<u32> = string.chars().map(key).collect();
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i + needle.len() <= chars.len() {
            let here = chars[i..i + needle.len()].iter().map(|&(_, c)| key(c));
            if here.eq(needle.iter().copied()) {
                let last = chars[i + needle.len() - 1];
                out.push(chars[i].0..last.0 + last.1.len_utf8());
                i += needle.len();
            } else {
                i += 1;
            }
        }
        out
    }

    proptest! {
        #[test]
        fn matches_agree_with_a_naive_scan(
            text in "[abAB\u{e9}\u{c9} ]{0,80}",
            string in "[abA\u{e9}]{1,4}",
        ) {
            prop_assert_eq!(all(&text, &string), naive(&text, &string));
        }

        #[test]
        fn forward_and_backward_find_neighbours(
            text in "[ab ]{0,80}",
            string in "[ab]{1,3}",
            at in 0usize..80,
        ) {
            let at = at.min(text.len());
            let every = naive(&text, &string);
            // Forward: the first match of a fresh scan from `at`.
            let expected = naive(&text[at..], &string)
                .first()
                .map(|range| range.start + at..range.end + at);
            let found = match forward(&text, &string, at) {
                Found::Match(range) => Some(range),
                _ => None,
            };
            prop_assert_eq!(found, expected);
            // Backward: some match that ends by `at`, and none of the scan's ends later.
            if let Found::Match(range) = backward(&text, &string, at) {
                prop_assert!(range.end <= at);
                let later = every.iter().any(|other| other.end <= at && other.start > range.start);
                prop_assert!(!later);
            } else {
                prop_assert!(every.iter().all(|other| other.end > at));
            }
        }
    }
}
