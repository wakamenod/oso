//! Tests of query-replace's regexps (ADR 0113): smart case, errors, the matches a search goes
//! through, its budget and its stops, the replacement, a line kept across edits, the
//! highlights, and migemo's regexps (ADR 0116): a long line's limit, and that every
//! pattern compiles on a key.
//!
//! Must not depend on: `buffer` or `editor`.

#![expect(
    clippy::single_range_in_vec_init,
    reason = "lists of ranges, which may hold one"
)]

use std::ops::Range;

use proptest::prelude::*;

use super::{Line, LineTooLong, MAX_PATTERN_CHARS, Scan, compile, smart_case};
use crate::decode::InvalidBytes;
use crate::migemo;
use crate::rope::Rope;
use crate::search::{Found, Needle};

/// Every match of `pattern` in `text`, from its start, as `!` would take them.
/// A time a test's search never reaches.
fn later() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(60)
}

fn all(pattern: &str, text: &str) -> Vec<Range<usize>> {
    within(pattern, text, &[0..text.len()])
}

fn within(pattern: &str, text: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let rope = Rope::from(text);
    let regexp = compile(pattern).unwrap();
    let mut scan = Scan::new(&regexp, &rope, ranges, (0, false), usize::MAX, None);
    let mut found = Vec::new();
    while let Ok(Found::Match(next)) = scan.next(&|| false) {
        found.push(next);
    }
    found
}

#[test]
fn smart_case_looks_past_escapes() {
    assert!(!smart_case(r"\W+\p{Lu}foo"));
    assert!(!smart_case(r"\pLfoo"));
    assert!(!smart_case(r"\PNfoo"));
    assert!(!smart_case(r"\x41\x{4A}"));
    assert!(smart_case("Foo"));
    assert!(smart_case(r"\dA"));
    assert!(!smart_case(r"(?P<Name>foo)(?<Other>bar)"));
    assert!(smart_case(r"(?P<name>Foo)"));
    assert_eq!(all("foo", "x FOO"), [2..5]);
    assert_eq!(all("Foo", "x FOO Foo"), [6..9]);
    assert!(all("(?-i)foo", "FOO").is_empty());
}

#[test]
fn an_invalid_regexp_says_why_once() {
    let error = compile("(").unwrap_err();
    assert!(error.starts_with("Invalid regexp: "), "{error}");
    assert!(!error.contains("error:"), "{error}");
    let long = "a".repeat(MAX_PATTERN_CHARS + 1);
    assert!(compile(&long).unwrap_err().contains("longer than"));
    // Unicode classes repeated a few dozen times fit.
    assert!(compile(r"\w{30}").is_ok());
    assert!(compile(r"[\p{L}\d]{25}").is_ok());
}

#[test]
fn a_match_keeps_to_its_line() {
    assert_eq!(all("^b", "ab\nbc"), [3..4]);
    assert_eq!(all("b$", "ab\nbc"), [1..2]);
    assert!(all(r"b\nb", "ab\nbc").is_empty());
    assert_eq!(all("c", "a\r\nc"), [3..4]);
}

#[test]
fn empty_matches_go_as_replace_all_takes_them() {
    // One right where a match ended is skipped; one at the end counts.
    assert_eq!(all(r"\s*$", "a  \nb"), [1..3, 5..5]);
    assert_eq!(all("a*", "baab"), [0..0, 1..3, 4..4]);
    assert_eq!(all("x*", "xa"), [0..1, 2..2]);
    assert_eq!(all("$", "a\r\nb"), [1..1, 4..4]);
}

#[test]
fn a_range_is_its_own_haystack_as_a_narrowed_search() {
    // A greedy match stops at the range's end.
    assert_eq!(within("a.*|c", "ac x", &[0..2]), [0..2]);
    assert_eq!(within("a+", "aaaa", &[0..2]), [0..2]);
    // Two ranges on one line are each searched.
    assert_eq!(within("a+", "aaaa", &[0..1, 2..4]), [0..1, 2..4]);
}

#[test]
fn a_range_s_end_inside_a_line_is_neither_a_line_s_end_nor_a_word_s() {
    assert!(within(r"\s+$", "foo bar", &[0..4]).is_empty());
    assert!(within(r"\w+$", "foo.bar", &[0..3]).is_empty());
    assert_eq!(within(r"\b", "abc def", &[0..2]), [0..0]);
    assert!(within("a+$", "aaaa", &[0..2]).is_empty());
    // Where the text after it makes them hold, they do.
    assert_eq!(within(r"\w+\b", "foo.bar", &[0..3]), [0..3]);
    assert_eq!(within(r"\s+$", "foo  ", &[0..5]), [3..5]);
    // A match that cannot end at the range's end gives way to a shorter one.
    assert_eq!(within("a+$|a", "aaab", &[0..2]), [0..1, 1..2]);
    assert_eq!(within(r"\w+\b|\w", "abcd", &[0..2]), [0..1, 1..2]);
    // An empty match at its end counts.
    assert_eq!(within(r"x*", "ab", &[1..1]), [1..1]);
    // A `(?x)` pattern's comment.
    assert_eq!(within("(?x) a+ # as", "aaa", &[0..2]), [0..2]);
}

#[test]
fn a_greedy_match_on_a_long_line_with_a_range_ending_in_it_is_quick() {
    let text = "a".repeat(200 * 1024);
    let started = std::time::Instant::now();
    assert_eq!(within("a+", &text, &[0..100 * 1024]), [0..100 * 1024]);
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn every_line_costs_toward_the_budget() {
    let rope = Rope::from("\n".repeat(100_000).as_str());
    let regexp = compile("x").unwrap();
    let ranges = [0..rope.len()];
    let mut scan = Scan::new(&regexp, &rope, &ranges, (0, false), 256 * 1024, None);
    assert!(matches!(scan.next(&|| false), Ok(Found::Stopped { .. })));
    assert!(scan.place().0 < rope.len(), "it stopped on the way");
}

#[test]
fn a_search_on_one_long_line_stops_when_asked() {
    let rope = Rope::from("a".repeat(1024 * 1024).as_str());
    let regexp = compile("a").unwrap();
    let ranges = [0..rope.len()];
    let mut scan = Scan::new(&regexp, &rope, &ranges, (0, false), usize::MAX, None);
    let asked = std::cell::Cell::new(false);
    let stop = || asked.get();
    // The line is read, and then the search is asked to stop while it finds match after match.
    assert_eq!(scan.next(&stop), Ok(Found::Match(0..1)));
    asked.set(true);
    let mut found = 0;
    loop {
        match scan.next(&stop) {
            Ok(Found::Match(_)) => found += 1,
            Ok(Found::Stopped { .. }) => break,
            other => panic!("{other:?}"),
        }
    }
    assert!(found < 10_000, "stopped soon, at {found}");
}

#[test]
fn a_line_longer_than_the_rule_stops_the_search() {
    let rope = Rope::from("short\nlong line\n");
    let mut regexp = compile("q").unwrap();
    regexp.set_longest_line(6);
    let ranges = [0..rope.len()];
    let mut scan = Scan::new(&regexp, &rope, &ranges, (0, false), usize::MAX, None);
    assert_eq!(scan.next(&|| false), Err(LineTooLong(6)));
}

#[test]
fn groups_expand_in_the_replacement() {
    let rope = Rope::from("x key=value");
    let regexp = compile(r"(\w+)=(?P<v>\w+)").unwrap();
    assert_eq!(regexp.expand(&rope, &(2..11), "${v}:$1 $$"), "value:key $");
    // At a line's start, `^` holds as it did when the match was found.
    let rope = Rope::from("ab");
    let regexp = compile("^a").unwrap();
    assert_eq!(regexp.expand(&rope, &(0..1), "[$0]"), "[a]");
}

#[test]
fn a_kept_line_follows_its_own_edits() {
    let rope = Rope::from("x ab ab\nnext");
    let regexp = compile("ab").unwrap();
    let ranges = [0..rope.len()];
    let mut scan = Scan::new(&regexp, &rope, &ranges, (0, false), usize::MAX, None);
    assert_eq!(scan.next(&|| false), Ok(Found::Match(2..4)));
    let line = scan.into_line().unwrap();
    let line: Line = line.edited(&[(2..4, "c"), (5..7, "dd")]);
    assert_eq!(line.text, "x c dd");
    assert_eq!(line.next, 7);
}

#[test]
fn highlights_look_around_the_view_and_leave_a_cut_match_out() {
    let text = format!("{}a1 a2 a3", "z".repeat(1000));
    let rope = Rope::from(text.as_str());
    let regexp = compile(r"a\d").unwrap();
    let (mut out, mut line) = (Vec::new(), super::LineRoom::default());
    // The view shows the end of the line.
    regexp.matches_in(
        &rope,
        &[1000..1008],
        (10, 64 * 1024),
        (Some(later()), &mut line),
        &mut out,
    );
    assert_eq!(out, [1000..1002, 1003..1005, 1006..1008]);
    // A window that cuts a match short leaves it out.
    let rope = Rope::from(format!("ab{}", "c".repeat(1000)).as_str());
    let regexp = compile("ab.*").unwrap();
    out.clear();
    regexp.matches_in(
        &rope,
        &[0..2],
        (10, 64 * 1024),
        (Some(later()), &mut line),
        &mut out,
    );
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn highlights_past_their_time_find_nothing_more() {
    let rope = Rope::from("a1 a2\na3\n");
    let regexp = compile(r"a\d").unwrap();
    let (mut out, mut line) = (Vec::new(), super::LineRoom::default());
    let gone = std::time::Instant::now();
    regexp.matches_in(
        &rope,
        &[0..5, 6..8],
        (10, 1024),
        (Some(gone), &mut line),
        &mut out,
    );
    assert!(out.is_empty(), "{out:?}");
    regexp.matches_in(
        &rope,
        &[0..5, 6..8],
        (10, 1024),
        (Some(later()), &mut line),
        &mut out,
    );
    assert_eq!(out, [0..2, 3..5, 6..8]);
}

#[test]
fn a_migemo_needle_on_a_long_line_reads_no_further_than_the_limit() {
    let needle = Needle::migemo("ka", false).unwrap();
    // A match lies far into a line of 8 MiB.
    let mut text = "x".repeat(8 * 1024 * 1024);
    text.push_str("か\n");
    let rope = Rope::from(text.as_str());
    let snapshot = (&rope, &InvalidBytes::new());
    let started = std::time::Instant::now();
    let (mut out, mut line) = (Vec::new(), super::LineRoom::default());
    let room = (Some(later()), &mut line);
    needle.matches_in(
        snapshot,
        &[0..rope.len()],
        (16, 1024 * 1024),
        room,
        &mut out,
    );
    assert!(out.is_empty());
    // A span within the limit is read.
    needle.matches_in(
        snapshot,
        &[rope.len() - 64..rope.len()],
        (16, 1024 * 1024),
        (Some(later()), &mut line),
        &mut out,
    );
    assert_eq!(out.len(), 1);
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
}

#[test]
fn a_migemo_match_at_a_cut_needs_no_regexps_for_a_cut() {
    let Some(Needle::Regexp(regexp)) = Needle::migemo("ka", false) else {
        panic!("ka gives kana");
    };
    // The range ends inside the line, just after a match.
    let rope = Rope::from("かか\n");
    let ranges = [0..3];
    let mut scan = Scan::new(&regexp, &rope, &ranges, (0, false), usize::MAX, None);
    scan.on_key();
    assert!(matches!(scan.next(&|| false), Ok(Found::Match(range)) if range == (0..3)));
    regexp.build_cut();
    assert!(!regexp.cut_built());
}

/// A romaji string of `spellings` from the table, joined, cut to [`migemo::MAX_CHARS`] at
/// a spelling's end, with `end` after it if that still fits.
fn romaji(spellings: &[&str], end: &str) -> String {
    let mut string = String::new();
    for spelling in spellings {
        if string.len() + spelling.len() > migemo::MAX_CHARS {
            break;
        }
        string.push_str(spelling);
    }
    if string.len() + end.len() <= migemo::MAX_CHARS {
        string.push_str(end);
    }
    string
}

/// Romaji strings of up to [`migemo::MAX_CHARS`]: spellings of the table at random, or one
/// of them repeated, which gives the largest patterns, with an unfinished end or none.
fn romaji_strings() -> impl Strategy<Value = String> {
    let spellings: Vec<&'static str> = migemo::spellings().collect();
    let any = proptest::sample::select(spellings);
    let mixed = proptest::collection::vec(any.clone(), 1..=200);
    let repeated = (any, 1..=256usize).prop_map(|(spelling, times)| vec![spelling; times]);
    let ends = proptest::sample::select(vec!["", "k", "n", "t", "ky", "-", "'"]);
    (prop_oneof![mixed, repeated], ends).prop_map(|(spellings, end)| romaji(&spellings, end))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every romaji string that has a migemo pattern compiles on a key, in either case: the
    /// string's length bound keeps its pattern within the key's size limit (ADR 0116).
    #[test]
    fn every_migemo_pattern_compiles_on_a_key(string in romaji_strings()) {
        if migemo::pattern(&string).is_some() {
            prop_assert!(Needle::migemo(&string, false).is_some(), "{} chars", string.len());
            prop_assert!(Needle::migemo(&string, true).is_some(), "{} chars", string.len());
        }
    }
}

/// The strings whose patterns are the largest compile on a key: a `-` and a spelling of
/// `ja`'s, `va`'s or `vo`'s size repeated, a doubled consonant, and the spellings of two kana
/// (ADR 0116).
#[test]
fn the_largest_migemo_patterns_compile_on_a_key() {
    let longest = |unit: &str| unit.repeat(migemo::MAX_CHARS / unit.len());
    let units = [
        "-ja", "-va", "-vo", "kk", "ss", "a", "zu", "ji", "mm", "n", "kyo",
    ];
    for unit in units {
        let string = longest(unit);
        assert!(migemo::pattern(&string).is_some(), "{unit}");
        assert!(Needle::migemo(&string, false).is_some(), "{unit}");
    }
}
