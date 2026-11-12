//! Tests of what regexp isearch adds to the regexps (ADR 0116): compiling on a key or the
//! worker, incomplete patterns, the search backward and what it costs, what a key leaves to
//! the worker, a newer job's place, the highlights' bound, and quoting.
//!
//! Must not depend on: `buffer` or `editor`.

use std::cell::Cell;
use std::ops::Range;
use std::time::Instant;

use proptest::prelude::*;

use super::{
    CompileError, Line, LineTooLong, On, PER_LINE, Regexp, Scan, compile_isearch, find_backward,
    quote,
};
use crate::rope::Rope;
use crate::search::{Found, Limits};

fn regexp(pattern: &str) -> Regexp {
    compile_isearch(pattern, true, On::Key).unwrap()
}

/// A worker's limits: none.
fn worker() -> (Limits, &'static dyn Fn() -> bool) {
    (Limits::NONE, &|| false)
}

/// A key's limits with `work` of budget and no time limit, as a test cannot keep to one.
fn key(work: usize) -> Limits {
    Limits {
        work,
        until: None,
        on_key: true,
    }
}

/// The last match that starts at `start_at_most` or before and ends by `before`, on the
/// worker.
fn back(pattern: &str, text: &str, before: usize, start_at_most: usize) -> Found {
    let rope = Rope::from(text);
    let compiled = compile_isearch(pattern, true, On::Worker).unwrap();
    find_backward(
        &compiled,
        &rope,
        (before, start_at_most),
        worker(),
        &mut None,
    )
    .unwrap()
}

#[test]
fn an_unclosed_pattern_is_incomplete_and_a_wrong_one_says_why() {
    for pattern in [
        "[", "[a-", "(", "(a|b", "a{2", r"\", "(?", "(?P<name", r"\p{", "x{",
    ] {
        let compiled = compile_isearch(pattern, true, On::Key);
        assert_eq!(compiled.err(), Some(CompileError::Incomplete), "{pattern}");
    }
    for (pattern, why) in [(")", "unopened group"), ("*", "missing expression")] {
        let Err(CompileError::Invalid(said)) = compile_isearch(pattern, true, On::Key) else {
            panic!("{pattern}");
        };
        assert!(
            said.starts_with("Invalid regexp: ") && said.contains(why),
            "{said}"
        );
    }
    // What it says never holds the pattern, whose text can be a secret's.
    let pattern = "(?P<token>hunter2)\n(?P<token>x)";
    let Err(CompileError::Invalid(said)) = compile_isearch(pattern, true, On::Key) else {
        panic!("compiled");
    };
    assert!(!said.contains("hunter2") && !said.contains('\n'), "{said}");
}

#[test]
fn a_pattern_too_large_or_heavy_for_a_key_is_the_worker_s() {
    for pattern in [
        r"\w{30}",
        &r"\p{Any}".repeat(146),
        r"[\x{0}-\x{10FFFF}]",
        r"\pL\p{Lu}",
        r"[\s\S]",
        r"a[\w\W]",
        "[!-\u{10FFFF}]",
        &r"[\s\S]".repeat(170),
    ] {
        let started = Instant::now();
        let compiled = compile_isearch(pattern, true, On::Key);
        assert_eq!(compiled.err(), Some(CompileError::ForWorker), "{pattern}");
        // Told apart before compiling, or by a compile that fails for size early.
        assert!(started.elapsed().as_millis() < 100, "{pattern}");
    }
    // One large class, an escaped backslash before a `p`, or classes that fold quickly, a
    // key takes.
    for pattern in [
        r"\p{Any}x",
        r"\\p\\p\\p",
        r"\S+[^\s]\W[a-z\d]",
        r"[^a]\[\S\]",
    ] {
        assert!(compile_isearch(pattern, true, On::Key).is_ok(), "{pattern}");
    }
    let full = compile_isearch(r"\w{30}", true, On::Worker).unwrap();
    assert!(!full.cut_built());
    let huge = r"\w{30}\w{30}\w{30}\w{30}";
    let Err(CompileError::Invalid(why)) = compile_isearch(huge, true, On::Worker) else {
        panic!("compiled");
    };
    assert!(why.starts_with("Invalid regexp"));
}

#[test]
fn a_key_leaves_the_regexps_for_a_cut_of_a_slow_regexp_to_the_worker() {
    // One large class takes a key about a millisecond to compile.
    let compiled = regexp(r"\p{Any}a|a+");
    let rope = Rope::from("aaa aaa");
    // A match that ends at the cut inside the line needs them.
    let found = find_backward(
        &compiled,
        &rope,
        (5, 5),
        (key(1 << 20), &|| false),
        &mut None,
    );
    assert_eq!(found, Ok(Found::Stopped { resume: 5 }));
    assert!(!compiled.cut_built());
    let found = find_backward(&compiled, &rope, (5, 5), worker(), &mut None);
    assert_eq!(found, Ok(Found::Match(4..5)));
    assert!(compiled.cut_built());
    // Once built, a key uses them, also in a copy made after.
    let copy = compiled.clone();
    let found = find_backward(&copy, &rope, (5, 5), (key(1 << 20), &|| false), &mut None);
    assert_eq!(found, Ok(Found::Match(4..5)));
    // A quick one, a key builds them.
    let quick = regexp("a+");
    let found = find_backward(&quick, &rope, (5, 5), (key(1 << 20), &|| false), &mut None);
    assert_eq!(found, Ok(Found::Match(4..5)));
}

#[test]
fn backward_takes_the_match_that_starts_last() {
    // Emacs tries every start, so one inside an earlier match is found.
    assert_eq!(back("aa", "aaa", 3, 3), Found::Match(1..3));
    assert_eq!(back("a+", "aaa aaa", 7, 7), Found::Match(6..7));
    assert_eq!(back("ab|b", "xab", 3, 3), Found::Match(2..3));
    // It starts at `start_at_most` or before.
    assert_eq!(back("a+", "aaa aaa", 7, 5), Found::Match(5..7));
    assert_eq!(back("o", "foo bar", 7, 7), Found::Match(2..3));
    assert_eq!(back("x", "foo bar", 7, 7), Found::None);
}

#[test]
fn backward_ends_by_its_bound_and_sees_the_text_after_it() {
    // `a+` stops at the bound, while `$` and `\b` there see what follows.
    assert_eq!(back("a+", "aaa aaa", 5, 5), Found::Match(4..5));
    assert_eq!(back(r"a+$", "aaa aaa", 5, 5), Found::None);
    assert_eq!(back(r"a+$", "aaa aaa", 7, 7), Found::Match(6..7));
    assert_eq!(back(r"\ba+\b", "aaa aaa", 5, 5), Found::Match(0..3));
}

#[test]
fn backward_goes_up_through_lines_one_at_a_time() {
    let text = "one foo\ntwo\nthree";
    assert_eq!(
        back("o+", text, text.len(), text.len()),
        Found::Match(10..11)
    );
    assert_eq!(
        back("f.o", text, text.len(), text.len()),
        Found::Match(4..7)
    );
    // Matches keep to a line: `\n` matches nothing, and `^` is a line's start.
    assert_eq!(back(r"o\nt", text, text.len(), text.len()), Found::None);
    assert_eq!(
        back("^t", text, text.len(), text.len()),
        Found::Match(12..13)
    );
    assert_eq!(back("^t", text, 12, 12), Found::Match(8..9));
}

/// How many times a search asked whether to stop, which it does before each look.
fn looks(text: &str, pattern: &str, backward: bool) -> (Found, usize) {
    let rope = Rope::from(text);
    let compiled = compile_isearch(pattern, true, On::Worker).unwrap();
    let asked = Cell::new(0);
    let stop = || {
        asked.set(asked.get() + 1);
        false
    };
    let len = rope.len();
    let found = if backward {
        find_backward(
            &compiled,
            &rope,
            (len, len),
            (Limits::NONE, &stop),
            &mut None,
        )
    } else {
        let whole = 0..len;
        let ranges = std::slice::from_ref(&whole);
        Scan::new(&compiled, &rope, ranges, (0, false), usize::MAX, None).next(&stop)
    };
    (found.unwrap(), asked.get())
}

#[test]
fn backward_halves_its_way_to_the_last_start_in_a_long_line() {
    let n = 64 * 1024;
    // Every start of the `a`s matches and reads to the line's end.
    let text = format!("{}{}", "a".repeat(n), "x".repeat(n));
    let (found, asked) = looks(&text, "a.*", true);
    assert_eq!(found, Found::Match(n - 1..2 * n));
    assert!(asked < 200, "{asked}");
    let text = format!("needle{}", "x".repeat(300_000));
    let (found, asked) = looks(&text, "needle", true);
    assert_eq!(found, Found::Match(0..6));
    assert!(asked < 64, "{asked}");
}

#[test]
fn a_newer_job_stops_a_search_within_a_look() {
    // Each match is short, but each look reads to the line's end.
    let text = format!("{}{}", "a".repeat(4000), "x".repeat(1 << 20));
    let rope = Rope::from(text.as_str());
    let compiled = compile_isearch("(a.*b)?", true, On::Worker).unwrap();
    let asked = Cell::new(0);
    let stop = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let whole = 0..rope.len();
    let ranges = std::slice::from_ref(&whole);
    let mut scan = Scan::new(&compiled, &rope, ranges, (0, false), usize::MAX, None);
    let mut found = 0;
    while let Ok(Found::Match(_)) = scan.next(&stop) {
        found += 1;
    }
    assert!(found <= 2, "{found}");
    let len = rope.len();
    let back = find_backward(
        &compiled,
        &rope,
        (len, len),
        (Limits::NONE, &|| true),
        &mut None,
    );
    assert_eq!(back, Ok(Found::Stopped { resume: len }));
}

#[test]
fn backward_stops_at_its_budget_where_it_can_go_on() {
    let text = format!("{}\nfoo\n{}", "a\n".repeat(50_000), "b\n".repeat(50_000));
    let rope = Rope::from(text.as_str());
    let compiled = regexp("^a");
    let len = text.len();
    let at_key = (key(64 * 1024), &|| false);
    let Ok(Found::Stopped { resume }) = find_backward(
        &compiled,
        &rope,
        (len, len),
        (at_key.0, at_key.1),
        &mut None,
    ) else {
        panic!("not stopped");
    };
    assert!(resume < len && resume > 50_000 * 2);
    let rest = find_backward(&compiled, &rope, (resume, len), worker(), &mut None);
    assert_eq!(rest, Ok(Found::Match(99_998..99_999)));
}

#[test]
fn backward_stops_at_its_time() {
    let text = format!("{}needle", "x".repeat(2_000_000));
    let rope = Rope::from(text.as_str());
    let compiled = regexp("needle$|^q");
    let len = text.len();
    let past = Limits {
        until: Some(Instant::now()),
        ..Limits::NONE
    };
    let found = find_backward(
        &compiled,
        &rope,
        (len - 6, len - 6),
        (past, &|| false),
        &mut None,
    );
    assert_eq!(found, Ok(Found::Stopped { resume: len - 6 }));
}

#[test]
fn a_key_leaves_a_line_longer_than_its_budget_or_its_line_unread() {
    let text = format!("short\n{}foo\n", "x".repeat(10_000));
    let rope = Rope::from(text.as_str());
    let compiled = regexp("foo");
    let whole = 0..rope.len();
    let mut scan = Scan::new(
        &compiled,
        &rope,
        std::slice::from_ref(&whole),
        (0, false),
        4096,
        None,
    );
    scan.on_key();
    assert_eq!(scan.next(&|| false), Ok(Found::Stopped { resume: 6 }));
    let kept = scan.into_line().map(|line| line.start);
    assert_eq!(kept, Some(0), "the long line is not read");
    let len = rope.len();
    let found = find_backward(
        &compiled,
        &rope,
        (len - 1, len - 1),
        (key(4096), &|| false),
        &mut None,
    );
    assert_eq!(found, Ok(Found::Stopped { resume: len - 1 }));
    // With room for it, and for looking in it, it is read.
    let room = key(4 * (10_003 + PER_LINE));
    let found = find_backward(
        &compiled,
        &rope,
        (len - 1, len - 1),
        (room, &|| false),
        &mut None,
    );
    assert_eq!(found, Ok(Found::Match(10_006..10_009)));
    // A line longer than a key looks in goes to the worker, whatever the budget.
    let text = format!("{}foo", "x".repeat(100_000));
    let rope = Rope::from(text.as_str());
    let len = rope.len();
    let found = find_backward(
        &compiled,
        &rope,
        (len, len),
        (key(usize::MAX), &|| false),
        &mut None,
    );
    assert_eq!(found, Ok(Found::Stopped { resume: len }));
}

#[test]
fn a_kept_line_is_not_read_again() {
    let text = format!("{}foo foo", "x".repeat(10_000));
    let rope = Rope::from(text.as_str());
    let compiled = regexp("foo");
    let mut kept: Option<Line> = None;
    let len = rope.len();
    let first = find_backward(&compiled, &rope, (len, len), worker(), &mut kept);
    assert_eq!(first, Ok(Found::Match(len - 3..len)));
    assert_eq!(kept.as_ref().map(|line| line.start), Some(0));
    // With the line kept, a budget too small to read it finds the one before.
    let small = key(4096);
    let next = find_backward(
        &compiled,
        &rope,
        (len - 3, len - 3),
        (small, &|| false),
        &mut kept,
    );
    assert_eq!(next, Ok(Found::Match(len - 7..len - 4)));
}

#[test]
fn backward_stops_at_a_line_too_long() {
    let mut compiled = regexp("o");
    compiled.set_longest_line(3);
    let rope = Rope::from("foo\nlonger\nbar");
    let found = find_backward(&compiled, &rope, (14, 14), worker(), &mut None);
    assert_eq!(found, Err(LineTooLong(4)));
}

#[test]
fn highlights_read_around_the_view_and_within_their_budget() {
    let text = format!("{} ab ab", "x".repeat(1000));
    let rope = Rope::from(text.as_str());
    let shown = std::slice::from_ref(&(1001..1006));
    let (mut out, mut line) = (Vec::new(), String::new());
    let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
    // `^` holds only at the line's start, though what is read starts near the view.
    regexp("^ab").matches_in(&rope, shown, (100, 1 << 20), (later, &mut line), &mut out);
    assert!(out.is_empty());
    regexp(r"\bab\b").matches_in(&rope, shown, (100, 1 << 20), (later, &mut line), &mut out);
    assert_eq!(out, [1001..1003, 1004..1006]);
    // Past the budget, nothing more is read.
    out.clear();
    regexp("ab").matches_in(&rope, shown, (100, 8), (later, &mut line), &mut out);
    assert!(out.is_empty());
}

#[test]
fn quoting_never_cuts_an_escape() {
    assert_eq!(quote("a.b", 10), r"a\.b");
    assert_eq!(quote("a.b", 2), "a");
    assert_eq!(quote("a.b", 3), r"a\.");
    assert_eq!(quote("日[", 2), "日");
}

/// The match that starts last at `start_at_most` or before in `text`, with `start_at_most`
/// on a line that ends by the text's end: each start tried from there back, as Emacs does.
fn naive_back(pattern: &str, text: &str, start_at_most: usize) -> Option<Range<usize>> {
    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .unwrap();
    let mut line_start = text[..start_at_most].rfind('\n').map_or(0, |at| at + 1);
    let mut last = start_at_most;
    loop {
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |at| line_start + at);
        let line = &text[line_start..line_end];
        for start in (0..=last - line_start)
            .rev()
            .filter(|&at| line.is_char_boundary(at))
        {
            if let Some(found) = regex.find_at(line, start)
                && found.start() == start
            {
                return Some(line_start + found.start()..line_start + found.end());
            }
        }
        if line_start == 0 {
            return None;
        }
        last = line_start - 1;
        line_start = text[..last].rfind('\n').map_or(0, |at| at + 1);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Lines longer than the first window, so that the windows double.
    #[test]
    fn backward_agrees_on_long_lines(
        text in "[ab x]{200,1500}",
        pattern in prop::sample::select(vec!["ab", "b+", "a.*", r"\bab", "x(a|b)"]),
        at in 0usize..1500,
    ) {
        let at = at.min(text.len());
        let found = match back(pattern, &text, text.len(), at) {
            Found::Match(range) => Some(range),
            _ => None,
        };
        prop_assert_eq!(found, naive_back(pattern, &text, at));
    }
}

proptest! {
    #[test]
    fn backward_agrees_with_trying_each_start(
        text in "[ab é日\n]{0,60}",
        pattern in prop::sample::select(vec![
            "a", "ab*", "b+", "a|ab", "^a", "b$", r"\bab", "a*", "(a|b)b", "é.", "日|b",
        ]),
        at in 0usize..200,
    ) {
        let mut at = at.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        let found = match back(pattern, &text, text.len(), at) {
            Found::Match(range) => Some(range),
            _ => None,
        };
        prop_assert_eq!(found, naive_back(pattern, &text, at));
    }
}
