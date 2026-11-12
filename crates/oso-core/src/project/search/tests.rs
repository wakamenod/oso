//! Tests of a project search's query: components matched per line, block search, highlights,
//! narrowing and the text a row shows.
//!
//! Must not depend on: `editor`, threads or the file system.

use super::{LineHit, LineQuery, MAX_ROW_BYTES, compile, row_text};

fn hits(input: &str, block: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    LineQuery::parse(input).find_in_block(block.as_bytes(), |hit| {
        found.push((hit.line, block[hit.range].to_owned()));
    });
    found
}

#[test]
fn a_line_matches_every_component_in_any_order() {
    let block = "fn main() {\n    let total = sum(a, b);\n    sum(total)\n}\n";
    assert_eq!(
        hits("sum total", block),
        [
            (1, "    let total = sum(a, b);".into()),
            (2, "    sum(total)".into())
        ]
    );
    assert_eq!(
        hits("total let", block),
        [(1, "    let total = sum(a, b);".into())]
    );
    assert!(hits("main total", block).is_empty(), "both on one line");
}

#[test]
fn an_excluded_component_drops_the_lines_it_matches() {
    let block = "use std::fmt;\nuse std::io;\nuse crate::x;\n";
    assert_eq!(hits("use !std", block), [(2, "use crate::x;".into())]);
    assert_eq!(hits("use !io !fmt", block), [(2, "use crate::x;".into())]);
}

#[test]
fn case_is_ignored_unless_the_input_holds_an_upper_case_letter() {
    let block = "Error\nerror\nERROR\n";
    assert_eq!(hits("error", block).len(), 3);
    assert_eq!(hits("Error", block), [(0, "Error".into())]);
    // Over the whole input: one upper-case letter anywhere keeps every component's case.
    assert!(hits("error X", block).is_empty());
}

#[test]
fn a_component_is_a_literal_and_an_expression_and_equals_keeps_it_literal() {
    let block = "a+b\naab\nfoo(\nx = 1;\n";
    // `a+b` as an expression matches `aab`, and as a literal `a+b`.
    assert_eq!(hits("a+b", block).len(), 2);
    assert_eq!(hits("=a+b", block), [(0, "a+b".into())]);
    // An expression that does not compile is a literal.
    assert_eq!(hits("foo(", block), [(2, "foo(".into())]);
    // `^` and `$` are a line's start and end, and other affixes are text.
    assert_eq!(hits("^aa", block), [(1, "aab".into())]);
    assert_eq!(hits("1;$", block), [(3, "x = 1;".into())]);
    assert_eq!(hits("x =", block), [(3, "x = 1;".into())]);
    assert_eq!(hits(",xy", ",xy\n"), [(0, ",xy".into())]);
}

#[test]
fn an_expression_that_compiles_only_joined_to_its_literal_is_a_literal() {
    let block = "a\nb\na)|(b\n)(\n";
    // Neither compiles alone, so each matches only itself, though the two joined would.
    assert_eq!(hits("a)|(b", block), [(2, "a)|(b".into())]);
    assert_eq!(hits(")(", block), [(3, ")(".into())]);
}

#[test]
fn an_expression_whose_comment_runs_to_its_end_is_still_an_expression() {
    // `(?x)` ignores spaces and reads `#` to the line's end as a comment: `(?x)a b #x` is `ab`,
    // and as a literal itself.
    let block = "ab\na b\n(?x)a b #x\n";
    assert_eq!(
        hits("(?x)a\\ b\\ #x", block),
        [(0, "ab".into()), (2, "(?x)a b #x".into())]
    );
}

#[test]
fn a_comment_does_not_end_inside_the_literal_at_its_line_break() {
    // A yanked line break: the comment would end at the literal's, and read the rest of the
    // literal as `(?x)` does, matching neither.
    let pattern = "x\n(?x)y #";
    let (regex, expression) = compile(pattern, true, false).unwrap();
    assert!(expression);
    assert!(regex.is_match(pattern.as_bytes()), "as a literal");
    assert!(regex.is_match(b"x\ny"), "as an expression");
}

#[test]
fn a_component_that_is_no_expression_narrows_as_a_literal() {
    let narrows =
        |before: &str, after: &str| LineQuery::parse(after).narrows(&LineQuery::parse(before));
    assert!(narrows("foo(", "foo(x"));
    assert!(!narrows("foo(", "foo()"), "now an expression too");
}

#[test]
fn a_line_break_and_a_carriage_return_are_no_part_of_a_line() {
    let block = "one\r\ntwo\r\nthree";
    assert_eq!(hits("two$", block), [(1, "two".into())]);
    assert_eq!(hits("thr", block), [(2, "three".into())]);
    // An expression that would cross lines matches within one only.
    assert!(hits("one\\s+two", block).is_empty());
}

#[test]
fn line_numbers_count_every_line_before_the_hit() {
    let block = "x\n\n\nneedle\nx\nneedle\n";
    assert_eq!(
        hits("needle", block),
        [(3, "needle".into()), (5, "needle".into())]
    );
    let mut found = Vec::new();
    LineQuery::parse("ee").find_in_block(b"aa\nneedle", |hit| found.push(hit));
    assert_eq!(
        found,
        [LineHit {
            line: 1,
            range: 3..9,
            first: 1..3
        }]
    );
}

#[test]
fn a_search_starts_at_two_characters_of_a_component_not_excluded() {
    assert!(!LineQuery::parse("").starts());
    assert!(!LineQuery::parse("a").starts());
    assert!(!LineQuery::parse("!ab").starts());
    assert!(!LineQuery::parse("a b !cd").starts());
    assert!(LineQuery::parse("ab").starts());
    assert!(LineQuery::parse("日本").starts());
    assert!(
        !LineQuery::parse(&"a ".repeat(40)).starts(),
        "too many components"
    );
    assert!(!LineQuery::parse(&"a".repeat(2000)).starts(), "too long");
}

#[test]
fn a_longer_literal_narrows_and_a_changed_expression_does_not() {
    let narrows =
        |after: &str, before: &str| LineQuery::parse(after).narrows(&LineQuery::parse(before));
    assert!(narrows("foob", "foo"));
    assert!(narrows("foo bar", "foo"));
    assert!(narrows("Foo", "foo"), "case may stop being ignored");
    assert!(!narrows("foo", "Foo"), "and never start");
    assert!(!narrows("fo", "foo"));
    assert!(
        !narrows("foo !b", "foo !ba"),
        "an exclusion that takes less keeps more"
    );
    assert!(!narrows("a.*bc", "a.*b"));
    // A literal read as an expression too finds more, and the other way fewer.
    assert!(!narrows("a.c", "=a.c"));
    assert!(!narrows("=a.c", "a.c"));
    assert!(!narrows("!=a.c", "!a.c"));
    assert!(narrows("a.*b x", "a.*b"));
}

#[test]
fn highlights_are_the_matches_of_the_components_joined_on_characters() {
    let query = LineQuery::parse("ab !zz cd");
    let mut out = Vec::new();
    query.highlights("xabcdx ab", &mut out);
    assert_eq!(out, [1..5, 7..9]);
    // A byte pattern that would land inside a character is widened to it.
    let query = LineQuery::parse("(?-u:\\xA5)x");
    query.highlights("¥x", &mut out);
    assert!(
        out.iter()
            .all(|range| "¥x".is_char_boundary(range.start) && "¥x".is_char_boundary(range.end))
    );
}

#[test]
fn a_row_shows_at_most_its_bytes_around_the_first_match() {
    let (text, column) = row_text(b"  let x = needle;", 10..16);
    assert_eq!((&*text, column), ("  let x = needle;", 10));
    let long = format!("{}needle{}", "a".repeat(1000), "b".repeat(1000));
    let (text, column) = row_text(long.as_bytes(), 1000..1006);
    assert_eq!(column, 1000);
    assert!(text.starts_with('…') && text.ends_with('…'));
    assert!(text.contains("needle"));
    let shown = text.trim_matches('…');
    assert_eq!(shown.len(), MAX_ROW_BYTES);
    // A match within the first bytes shows the line from its start.
    let (text, _) = row_text(long.as_bytes(), 3..5);
    assert!(text.starts_with("aaa") && text.ends_with('…'));
}

#[test]
fn a_row_cuts_at_characters_and_reads_invalid_bytes_as_a_buffer_does() {
    let line = "あ".repeat(400);
    let (text, _) = row_text(line.as_bytes(), 0..3);
    let shown = text.trim_end_matches('…');
    assert!(shown.len() <= MAX_ROW_BYTES && shown.chars().all(|c| c == 'あ'));
    // Each invalid byte is one character, in the text and in the column.
    let (text, column) = row_text(b"\xff\xfeab", 2..4);
    assert_eq!((&*text, column), ("\u{FFFD}\u{FFFD}ab", 2));
}

#[test]
fn debug_says_nothing_of_the_input() {
    let debug = format!("{:?}", LineQuery::parse("hunter2 secret"));
    assert!(!debug.contains("hunter2") && !debug.contains("secret"));
}

#[test]
fn a_block_tells_its_line_breaks_and_a_pattern_across_lines_costs_one_pass() {
    let query = LineQuery::parse("ee");
    assert_eq!(query.find_in_block(b"aa\nneedle\nx\n", |_| {}), 3);
    assert_eq!(query.find_in_block(b"no match\nhere", |_| {}), 1);
    assert_eq!(LineQuery::parse("!x").find_in_block(b"a\nb\n", |_| {}), 2);
    // A greedy expression that would run to the block's end from each line: lines of spaces.
    let block = " \n".repeat(200_000);
    let started = std::time::Instant::now();
    let mut found = 0;
    LineQuery::parse("\\s+x").find_in_block(block.as_bytes(), |_| found += 1);
    assert_eq!(found, 0);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "linear"
    );
    // The longest component drives, and the others are matched on its lines.
    assert_eq!(
        hits("in needle", "in a\nneedle in\n"),
        [(1, "needle in".to_owned())]
    );
}

#[test]
fn no_line_is_skipped_whatever_end_the_engine_gives_and_far_ends_stay_linear() {
    // A Unicode word boundary over text that is not ASCII makes the engine fall back.
    let block = "é\na\nab\nb\n";
    assert_eq!(
        hits("\\ba[^z]*b", block),
        [(2, "ab".to_owned())],
        "the line whose own match ends first"
    );
    let mut lines = String::from("é\n");
    for _ in 0..20_000 {
        lines.push_str("a\n");
    }
    let started = std::time::Instant::now();
    let found = hits("\\ba[^z]*q|\\bzz", &lines);
    assert!(found.is_empty());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "linear"
    );
}

#[test]
fn a_match_that_crosses_lines_is_not_looked_through_again_from_each_of_them() {
    // A `;` every 400 short lines: each line starts a match that ends at the next `;`.
    let mut block = String::new();
    while block.len() < 1024 * 1024 {
        for _ in 0..399 {
            block.push_str("abcdefghi\n");
        }
        block.push_str("abcdefgh;\n");
    }
    let started = std::time::Instant::now();
    let mut found = 0;
    LineQuery::parse("[^;]+;").find_in_block(block.as_bytes(), |_| found += 1);
    assert_eq!(found, block.matches(';').count());
    // Debug builds are slow; looking each line's 4 KiB through again took seconds here.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}
