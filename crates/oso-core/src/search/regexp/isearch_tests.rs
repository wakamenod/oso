//! Tests of what regexp isearch adds to the regexps (ADR 0116): compiling within a key's
//! limit, incomplete patterns, the search backward and its budget, and a long line left to
//! the worker.
//!
//! Must not depend on: `buffer` or `editor`.

use std::time::Instant;

use proptest::prelude::*;

use super::{
    Invalid, KEY_SIZE_LIMIT, LineTooLong, PER_LINE, Regexp, Scan, compile_isearch, find_backward,
};
use crate::rope::Rope;
use crate::search::Found;

fn regexp(pattern: &str) -> Regexp {
    compile_isearch(pattern, true, (KEY_SIZE_LIMIT, false)).unwrap()
}

/// The last match that starts at `start_at_most` or before and ends by `before`, unbounded.
fn back(pattern: &str, text: &str, before: usize, start_at_most: usize) -> Found {
    let rope = Rope::from(text);
    let limits = (usize::MAX, None, false);
    find_backward(
        &regexp(pattern),
        &rope,
        (before, start_at_most),
        limits,
        &|| false,
    )
    .unwrap()
}

#[test]
fn an_unclosed_pattern_is_incomplete_and_a_wrong_one_says_why() {
    for pattern in [
        "[", "[a-", "(", "(a|b", "a{2", r"\", "(?", "(?P<name", r"\p{", "x{",
    ] {
        let compiled = compile_isearch(pattern, true, (KEY_SIZE_LIMIT, false));
        assert_eq!(compiled.err(), Some(Invalid::Incomplete), "{pattern}");
    }
    for (pattern, why) in [(")", "unopened group"), ("*", "missing expression")] {
        let Err(Invalid::Wrong(said)) = compile_isearch(pattern, true, (KEY_SIZE_LIMIT, false))
        else {
            panic!("{pattern}");
        };
        assert!(
            said.starts_with("Invalid regexp: ") && said.contains(why),
            "{said}"
        );
        assert!(!said.contains('\n'));
    }
}

#[test]
fn a_pattern_too_large_for_a_key_compiles_at_the_full_limit() {
    let big = r"\w{30}";
    assert_eq!(
        compile_isearch(big, true, (KEY_SIZE_LIMIT, false)).err(),
        Some(Invalid::TooBig)
    );
    let full = compile_isearch(big, true, (usize::MAX, true)).unwrap();
    assert!(full.cut_built(), "the worker builds the cut's too");
    // Past the full limit, it says why.
    let huge = r"\w{30}\w{30}\w{30}\w{30}";
    let Err(Invalid::Wrong(why)) = compile_isearch(huge, true, (usize::MAX, true)) else {
        panic!("compiled");
    };
    assert!(why.starts_with("Invalid regexp"));
}

#[test]
fn a_key_compiles_small_patterns_quickly_and_leaves_the_cut_regexps_for_later() {
    for pattern in [r"[\p{L}\d]{25}", r"(?i)\p{L}{20}x", r"\w{8}"] {
        let started = Instant::now();
        let compiled = compile_isearch(pattern, true, (KEY_SIZE_LIMIT, false));
        // Whether it compiles or is left to the worker, the key's compile is bounded.
        assert!(started.elapsed().as_millis() < 100, "{pattern}");
        if let Ok(compiled) = compiled {
            assert!(!compiled.cut_built());
        }
    }
    let compiled = regexp("a+");
    assert!(!compiled.cut_built());
    // A match at a cut inside a line needs them.
    let rope = Rope::from("aaa aaa");
    let found = find_backward(&compiled, &rope, (5, 5), (usize::MAX, None, false), &|| {
        false
    });
    assert_eq!(found, Ok(Found::Match(4..5)));
    assert!(compiled.cut_built());
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

#[test]
fn backward_finds_a_match_far_from_the_end_of_a_long_line() {
    let text = format!("needle{}", "x".repeat(300_000));
    let rope = Rope::from(text.as_str());
    let asked = std::cell::Cell::new(0);
    let limits = (usize::MAX, None, false);
    let stop = || {
        asked.set(asked.get() + 1);
        false
    };
    let found = find_backward(
        &regexp("needle"),
        &rope,
        (text.len(), text.len()),
        limits,
        &stop,
    );
    assert_eq!(found, Ok(Found::Match(0..6)));
    // Windows that double read the line a few times at most, so `stop` is asked about as
    // often as a forward search's.
    assert!(asked.get() < 64, "{}", asked.get());
}

#[test]
fn backward_stops_at_its_budget_where_it_can_go_on() {
    let text = format!("{}\nfoo\n{}", "a\n".repeat(50_000), "b\n".repeat(50_000));
    let rope = Rope::from(text.as_str());
    let compiled = regexp("^a");
    let limits = (64 * 1024, None, true);
    let len = text.len();
    let Ok(Found::Stopped { resume }) =
        find_backward(&compiled, &rope, (len, len), limits, &|| false)
    else {
        panic!("not stopped");
    };
    assert!(resume < len && resume > 50_000 * 2);
    let unbounded = (usize::MAX, None, false);
    let rest = find_backward(&compiled, &rope, (resume, len), unbounded, &|| false);
    assert_eq!(rest, Ok(Found::Match(99_998..99_999)));
}

#[test]
fn backward_stops_at_its_time_and_when_asked() {
    let text = format!("{}needle", "x".repeat(2_000_000));
    let rope = Rope::from(text.as_str());
    let compiled = regexp("needle$|^q");
    let len = text.len();
    let past = (usize::MAX, Some(Instant::now()), false);
    let found = find_backward(&compiled, &rope, (len - 6, len - 6), past, &|| false);
    assert_eq!(found, Ok(Found::Stopped { resume: len - 6 }));
    let unbounded = (usize::MAX, None, false);
    let found = find_backward(&compiled, &rope, (len - 6, len - 6), unbounded, &|| true);
    assert_eq!(found, Ok(Found::Stopped { resume: len - 6 }));
}

#[test]
fn a_key_leaves_a_line_longer_than_its_budget_unread() {
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
    scan.short_lines();
    assert_eq!(scan.next(&|| false), Ok(Found::Stopped { resume: 6 }));
    let kept = scan.into_line().map(|line| line.start);
    assert_eq!(kept, Some(0), "the long line is not read");
    // Backward too.
    let limits = (4096, None, true);
    let len = rope.len();
    let found = find_backward(&compiled, &rope, (len - 1, len - 1), limits, &|| false);
    assert_eq!(found, Ok(Found::Stopped { resume: len - 1 }));
    // With room for it, and for looking in it, it is read.
    let room = (4 * (10_003 + PER_LINE), None, true);
    let found = find_backward(&compiled, &rope, (len - 1, len - 1), room, &|| false);
    assert_eq!(found, Ok(Found::Match(10_006..10_009)));
}

#[test]
fn backward_stops_at_a_line_too_long() {
    let mut compiled = regexp("o");
    compiled.set_longest_line(3);
    let rope = Rope::from("foo\nlonger\nbar");
    let found = find_backward(
        &compiled,
        &rope,
        (14, 14),
        (usize::MAX, None, false),
        &|| false,
    );
    assert_eq!(found, Err(LineTooLong(4)));
}

/// The match that starts last at `start_at_most` or before in `text`, with `start_at_most`
/// on a line that ends by the text's end: each start tried from there back, as Emacs does.
fn naive_back(pattern: &str, text: &str, start_at_most: usize) -> Option<std::ops::Range<usize>> {
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
        for start in (0..=last - line_start).rev() {
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
    #[test]
    fn backward_agrees_with_trying_each_start(
        text in "[ab \n]{0,60}",
        pattern in prop::sample::select(vec!["a", "ab*", "b+", "a|ab", "^a", "b$", r"\bab", "a*", "(a|b)b"]),
        at in 0usize..61,
    ) {
        let at = at.min(text.len());
        let found = match back(pattern, &text, text.len(), at) {
            Found::Match(range) => Some(range),
            _ => None,
        };
        prop_assert_eq!(found, naive_back(pattern, &text, at));
    }
}
