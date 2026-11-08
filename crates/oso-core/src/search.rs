//! Literal search over a rope, for isearch (ADR 0082): a query built from a string with
//! smart case, and searches forward, backward, in a range and through the whole text.
//!
//! Must not depend on: `buffer`, `motion`, `editor`, threads or a log. The worker's
//! [`SearchJob`] is a plain function here, and the host runs it on its thread.
//!
//! - The text is read one character at a time with Knuth–Morris–Pratt, so the cost is
//!   linear in the text whatever the string, and a match may cross chunks.
//! - Case folds one character at a time: a character whose lowercase is one character
//!   becomes it, and any other stays. Width and characters are not folded.
//! - The U+FFFD of an invalid byte matches nothing (ADR 0040).
//! - Nothing here logs, and `Debug` prints no text of a query or a job: a search string can
//!   say something of a buffer that may hold secrets (ADR 0044).

use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::decode::InvalidBytes;
use crate::rope::Rope;
use crate::sum_tree::Bias;

/// The most characters a search string holds. What is added past it is cut, so that what a
/// frame looks at around the view stays small.
pub const MAX_QUERY_CHARS: usize = 1024;

/// How much text one key searches on the UI thread before the rest goes to a worker.
pub const KEY_BUDGET: usize = 256 * 1024;

/// The bytes a backward search reads at once, and a worker reads between looks at whether a
/// newer job has taken its place.
const BLOCK: usize = 64 * 1024;

/// What the U+FFFD of an invalid byte reads as: no character, so nothing matches it.
const INVALID: u32 = u32::MAX;

/// Whether `string` makes the search case-sensitive: it holds an uppercase letter.
pub fn smart_case(string: &str) -> bool {
    string.chars().any(char::is_uppercase)
}

/// `c` folded to its lowercase, if that is one character.
fn fold(c: char) -> u32 {
    if c.is_ascii() {
        return u32::from(c.to_ascii_lowercase());
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(lower), None) => u32::from(lower),
        _ => u32::from(c),
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
        let mut reversed = chars.clone();
        reversed.reverse();
        Some(Self {
            forward: Pattern::new(chars),
            backward: Pattern::new(reversed),
            case_sensitive,
        })
    }

    /// Whether case matters.
    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// The most bytes a match can take: four for each character, as a folded character can
    /// stand for a longer one (`K` for the Kelvin sign).
    pub fn max_bytes(&self) -> usize {
        self.forward.chars.len().saturating_mul(4)
    }

    /// What the character `c` at `offset` of the text reads as.
    fn key(&self, c: char, offset: usize, invalid: &InvalidBytes) -> u32 {
        if c == char::REPLACEMENT_CHARACTER && invalid.get(offset).is_some() {
            return INVALID;
        }
        if self.case_sensitive {
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
    matched: usize,
    /// The other end of each of the last characters fed, as a ring.
    ends: Vec<usize>,
    fed: usize,
}

impl<'p> Matcher<'p> {
    fn new(pattern: &'p Pattern) -> Self {
        Self {
            pattern,
            matched: 0,
            ends: vec![0; pattern.chars.len()],
            fed: 0,
        }
    }

    /// Feeds the character `key` with `other_end`, the end it would give a match it starts.
    /// Returns that end of the match it completes, if it does. The next match starts after
    /// it, so matches do not overlap.
    fn feed(&mut self, key: u32, other_end: usize) -> Option<usize> {
        let chars = &self.pattern.chars;
        let len = chars.len();
        self.ends[self.fed % len] = other_end;
        self.fed += 1;
        while self.matched > 0 && chars[self.matched] != key {
            self.matched = self.pattern.fail[self.matched - 1];
        }
        if chars[self.matched] == key {
            self.matched += 1;
        }
        if self.matched < len {
            return None;
        }
        self.matched = 0;
        Some(self.ends[(self.fed - len) % len])
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

/// Feeds the characters of `range` to `on_char`, first to last, with their offset, until it
/// returns false. Stops once `limit` bytes are read, or `stop` says so between blocks, and
/// returns the offset it had reached then.
fn walk_forward(
    text: &Rope,
    range: Range<usize>,
    limit: usize,
    stop: &dyn Fn() -> bool,
    mut on_char: impl FnMut(char, usize) -> bool,
) -> Option<usize> {
    let mut offset = text.clip_offset(range.start, Bias::Left);
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
            if !on_char(c, offset + index) {
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
    text: &Rope,
    range: Range<usize>,
    limit: usize,
    stop: &dyn Fn() -> bool,
    mut on_char: impl FnMut(char, usize) -> bool,
) -> Option<usize> {
    let start = text.clip_offset(range.start, Bias::Left);
    let mut high = text.clip_offset(range.end, Bias::Left).max(start);
    let mut read = 0usize;
    let mut chunks: Vec<&str> = Vec::new();
    while high > start {
        if read >= limit || (read > 0 && stop()) {
            return Some(high);
        }
        let low = text.clip_offset(high.saturating_sub(BLOCK).max(start), Bias::Left);
        chunks.clear();
        chunks.extend(text.chunks_in_range(low..high));
        let mut end = high;
        for chunk in chunks.iter().rev() {
            let base = end - chunk.len();
            for (index, c) in chunk.char_indices().rev() {
                if !on_char(c, base + index) {
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
    let mut matcher = Matcher::new(&query.forward);
    let mut found = None;
    let stopped = walk_forward(text, from..text.len(), limit, stop, |c, offset| {
        let key = query.key(c, offset, invalid);
        match matcher.feed(key, offset) {
            Some(start) => {
                found = Some(start..offset + c.len_utf8());
                false
            }
            None => true,
        }
    });
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
    let mut matcher = Matcher::new(&query.backward);
    let mut found = None;
    let stopped = walk_backward(text, 0..before, limit, stop, |c, offset| {
        let key = query.key(c, offset, invalid);
        match matcher.feed(key, offset + c.len_utf8()) {
            Some(end) if offset <= start_at_most => {
                found = Some(offset..end);
                false
            }
            _ => true,
        }
    });
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

/// The matches of `query` in `range`, into `out`, in order and not overlapping, as a scan
/// from its start finds them, at most `most` of them.
pub fn matches_in(
    text: &Rope,
    invalid: &InvalidBytes,
    query: &Query,
    range: Range<usize>,
    most: usize,
    out: &mut Vec<Range<usize>>,
) {
    if most == 0 {
        return;
    }
    let mut matcher = Matcher::new(&query.forward);
    walk_forward(text, range, usize::MAX, &|| false, |c, offset| {
        let key = query.key(c, offset, invalid);
        if let Some(start) = matcher.feed(key, offset) {
            out.push(start..offset + c.len_utf8());
            return out.len() < most;
        }
        true
    });
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
    let stopped = walk_forward(text, 0..text.len(), usize::MAX, stop, |c, offset| {
        let key = query.key(c, offset, invalid);
        if let Some(start) = matcher.feed(key, offset) {
            count.total += 1;
            if current.is_some_and(|current| start < current) {
                count.before += 1;
            }
        }
        true
    });
    stopped.is_none().then_some(count)
}

/// Where a worker goes on searching, as [`find_forward`] and [`find_backward`] take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find {
    Forward { from: usize },
    Backward { before: usize, start_at_most: usize },
}

/// What a worker found. `Debug` gives no offsets, as they can say something of a buffer
/// that may hold secrets.
#[derive(Clone, PartialEq, Eq)]
pub enum SearchReport {
    /// The search it went on with ended: at a match, or with none.
    Found {
        generation: u64,
        at: Option<Range<usize>>,
    },
    /// It counted the matches, for the match that starts at `at`.
    Counted {
        generation: u64,
        count: Count,
        at: Option<usize>,
    },
}

impl fmt::Debug for SearchReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, generation) = match self {
            Self::Found { generation, .. } => ("Found", generation),
            Self::Counted { generation, .. } => ("Counted", generation),
        };
        f.debug_struct(kind)
            .field("generation", generation)
            .finish_non_exhaustive()
    }
}

/// Work for a thread, over an O(1) snapshot of a buffer's text: a search to go on with, if
/// a key's budget ran out, and then a count. A newer job takes its place, and it stops
/// within a block once its generation is no longer the live one.
pub struct SearchJob {
    text: Rope,
    invalid: InvalidBytes,
    query: Query,
    find: Option<Find>,
    /// Where the current match starts, if `find` does not say.
    current: Option<usize>,
    generation: u64,
    live: Arc<AtomicU64>,
}

impl fmt::Debug for SearchJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No text and no offsets (ADR 0044).
        f.debug_struct("SearchJob")
            .field("generation", &self.generation)
            .field("finds", &self.find.is_some())
            .finish_non_exhaustive()
    }
}

impl SearchJob {
    /// A job of `generation`, which runs while `live` holds it.
    pub(crate) fn new(
        (text, invalid): (Rope, InvalidBytes),
        query: Query,
        find: Option<Find>,
        current: Option<usize>,
        (generation, live): (u64, Arc<AtomicU64>),
    ) -> Self {
        Self {
            text,
            invalid,
            query,
            find,
            current,
            generation,
            live,
        }
    }

    /// The job's generation, which its reports carry.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a newer job has taken its place, or the search has ended.
    pub fn superseded(&self) -> bool {
        self.live.load(Ordering::Relaxed) != self.generation
    }

    /// Does the work, and reports each part as it ends, unless superseded first.
    pub fn run(self, mut report: impl FnMut(SearchReport)) {
        let stop = || self.superseded();
        let (text, invalid, query) = (&self.text, &self.invalid, &self.query);
        let mut current = self.current;
        if let Some(find) = self.find {
            let found = match find {
                Find::Forward { from } => {
                    find_forward(text, invalid, query, from, usize::MAX, &stop)
                }
                Find::Backward {
                    before,
                    start_at_most,
                } => find_backward(
                    text,
                    invalid,
                    query,
                    before,
                    start_at_most,
                    usize::MAX,
                    &stop,
                ),
            };
            let at = match found {
                Found::Match(range) => Some(range),
                Found::None => None,
                Found::Stopped { .. } => return,
            };
            current = at.as_ref().map(|range| range.start);
            report(SearchReport::Found {
                generation: self.generation,
                at,
            });
        }
        if let Some(count) = count(text, invalid, query, current, &stop) {
            report(SearchReport::Counted {
                generation: self.generation,
                count,
                at: current,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
        matches_in(
            &rope,
            &InvalidBytes::new(),
            &query(string),
            0..rope.len(),
            usize::MAX,
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
            query("a"),
            Some(Find::Forward { from: 1 }),
            None,
            (3, live),
        );
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        assert_eq!(
            reports,
            [
                SearchReport::Found {
                    generation: 3,
                    at: Some(4..5)
                },
                SearchReport::Counted {
                    generation: 3,
                    count: Count {
                        total: 3,
                        before: 1
                    },
                    at: Some(4)
                },
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
            query("a"),
            None,
            None,
            (3, live),
        );
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        assert!(reports.is_empty());
    }

    #[test]
    fn debug_shows_no_text() {
        let q = query("hunter2");
        assert!(!format!("{q:?}").contains("hunter"));
    }

    #[test]
    fn a_query_is_cut_to_its_most_characters() {
        let long = "a".repeat(MAX_QUERY_CHARS + 10);
        assert_eq!(query(&long).max_bytes(), MAX_QUERY_CHARS * 4);
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
