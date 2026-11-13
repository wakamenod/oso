//! Tests of a project search's query: components matched per line, block search, highlights,
//! narrowing and the text a row shows.
//!
//! Must not depend on: `editor`, threads or the file system.

use std::sync::Arc;

use proptest::prelude::*;

use super::{LineHit, LineQuery, MAX_ROW_BYTES, compile, row_text};
use crate::migemo::{Dictionary, Table};

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
    // Literals with a digit, which are not romaji.
    assert!(narrows("f00b", "f00"));
    assert!(narrows("f00 b4r", "f00"));
    assert!(narrows("F00", "f00"), "case may stop being ignored");
    assert!(!narrows("f00", "F00"), "and never start");
    assert!(!narrows("f0", "f00"));
    assert!(
        !narrows("f00 !b1", "f00 !b12"),
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

/// A dictionary of `text`, in `migemo-dict`'s format, as the editor's first.
fn dictionary(text: &str) -> Table {
    Table {
        generation: 1,
        dictionary: Arc::new(Dictionary::new(text.to_owned())),
    }
}

/// The lines of `block` that `query` matches.
fn lines_of(query: &LineQuery, block: &str) -> Vec<String> {
    let mut found = Vec::new();
    query.find_in_block(block.as_bytes(), |hit| {
        found.push(block[hit.range].to_owned());
    });
    found
}

#[test]
fn a_romaji_component_matches_its_kana_in_every_form_and_itself() {
    let block = "かんじ\nカンジ\nｶﾝｼﾞ\nｋａｎｊｉ\nkanji\nKANJI\n漢字\nnothing\n";
    let query = LineQuery::parse("kanji");
    assert!(query.uses_migemo() && query.wants_dictionary());
    assert_eq!(
        lines_of(&query, block),
        ["かんじ", "カンジ", "ｶﾝｼﾞ", "ｋａｎｊｉ", "kanji", "KANJI"]
    );
    // With a dictionary, its words too.
    let words = dictionary("かんじ\t漢字\n");
    let query = LineQuery::parse_after("kanji", &LineQuery::empty(), true, Some(&words));
    assert!(query.uses_migemo() && !query.wants_dictionary());
    assert!(lines_of(&query, block).contains(&"漢字".to_owned()));
}

#[test]
fn equals_the_setting_case_and_letters_that_spell_nothing_keep_a_component_literal() {
    let block = "かんじ\nkanji\n";
    let literal = |query: &LineQuery| !query.uses_migemo() && lines_of(query, block) == ["kanji"];
    assert!(literal(&LineQuery::parse("=kanji")));
    assert!(literal(&LineQuery::parse_after(
        "kanji",
        &LineQuery::empty(),
        false,
        None
    )));
    // An upper-case letter is not romaji, and makes the case count over the whole input.
    assert!(!LineQuery::parse("Kanji").uses_migemo());
    // A letter that starts no spelling before the end.
    assert!(!LineQuery::parse("struct").uses_migemo());
    // A character an expression reads keeps it a literal and an expression.
    assert!(!LineQuery::parse("kan.i").uses_migemo());
    assert_eq!(lines_of(&LineQuery::parse("kan.i"), block), ["kanji"]);
}

#[test]
fn a_romaji_exclusion_leaves_out_its_kana_too() {
    let block = "a かんじ\na カンジ\na kanji\na other\n";
    assert_eq!(lines_of(&LineQuery::parse("a !kanji"), block), ["a other"]);
}

#[test]
fn a_romaji_component_narrows_where_the_old_one_matches_each_of_its_strings() {
    let words = dictionary("かんじ\t漢字\nかなだ\t加奈陀\n");
    let parse = |input: &str, words: Option<&Table>| {
        LineQuery::parse_after(input, &LineQuery::empty(), true, words)
    };
    // Typed a letter at a time, each string holds the one before's.
    for (old, new) in [("n", "na"), ("na", "nam"), ("nam", "name"), ("kan", "kanj")] {
        assert!(parse(new, None).narrows(&parse(old, None)), "{old} {new}");
        assert!(parse(new, Some(&words)).narrows(&parse(old, Some(&words))));
    }
    // A letter that spells no kana makes a literal, which the old pattern holds too.
    assert!(parse("st", None).narrows(&parse("s", None)));
    assert!(parse("kan x1", None).narrows(&parse("kan", None)));
    // `kana`'s `加奈陀` is no string of `kan` without its words, nor `kanji`'s `かんじ` of `kana`.
    assert!(!parse("kana", Some(&words)).narrows(&parse("kan", None)));
    assert!(!parse("kanji", None).narrows(&parse("kana", None)));
    // Every form of `kanji` holds `ji`'s, but `漢字` holds none.
    assert!(parse("kanji", None).narrows(&parse("ji", None)));
    assert!(!parse("kanji", Some(&words)).narrows(&parse("ji", Some(&words))));
    // A dictionary that came adds words; one that went leaves the strings the old one had.
    assert!(!parse("kan", Some(&words)).narrows(&parse("kan", None)));
    assert!(parse("kan", None).narrows(&parse("kan", Some(&words))));
    // Literal again, as the setting turned off: still within the pattern's strings.
    let off = LineQuery::parse_after("kan", &LineQuery::empty(), false, None);
    assert!(off.narrows(&parse("kan", None)));
    assert!(
        !parse("kan", None).narrows(&off),
        "the literal holds no kana"
    );
    // Case may stop being ignored, never start.
    assert!(parse("kanX", None).narrows(&parse("kan", None)));
    assert!(!parse("kan", None).narrows(&parse("Kan", None)));
    // An exclusion narrows only as it is.
    assert!(!parse("a !kanj", None).narrows(&parse("a !kan", None)));
    assert!(parse("a !kan", None).narrows(&parse("a !kan", None)));
}

#[test]
fn a_pattern_of_too_many_strings_never_narrows() {
    let parse = |input: &str| LineQuery::parse_after(input, &LineQuery::empty(), true, None);
    // Each `zu` is `ず` or `づ`, so ten of them are 1024 strings in each form.
    let many = "zu".repeat(10);
    assert!(!parse(&format!("{many}z")).narrows(&parse(&many)));
    let few = "zu".repeat(3);
    assert!(parse(&format!("{few}z")).narrows(&parse(&few)));
}

#[test]
fn a_component_the_input_before_had_keeps_what_it_compiled() {
    let words = dictionary("かんじ\t漢字\n");
    let parse = |input: &str, before: &LineQuery, words: Option<&Table>| {
        LineQuery::parse_after(input, before, true, words)
    };
    let first = parse("kan a.*b f00", &LineQuery::empty(), None);
    assert_eq!(first.fresh(), [true, true, true]);
    let second = parse("kan a.*b f00 ji", &first, None);
    assert_eq!(second.fresh(), [false, false, false, true]);
    // A component repeated compiles once.
    assert_eq!(
        parse("kan kan !kan", &LineQuery::empty(), None).fresh(),
        [true, false, false]
    );
    // A pasted input compiles each new component once, and the next key only what changed.
    let pasted = parse("kan a.*b f00 ji zu do ma", &second, None);
    assert_eq!(
        pasted.fresh(),
        [false, false, false, false, true, true, true]
    );
    // A dictionary that came compiles migemo's components again, and only them.
    let third = parse("kan a.*b f00 ji", &second, Some(&words));
    assert_eq!(third.fresh(), [true, false, false, true]);
    // A case that changes compiles everything again, and `=` before one compiles it again;
    // `!` keeps what it compiled.
    assert_eq!(
        parse("kan a.*b F00", &third, Some(&words)).fresh(),
        [true; 3]
    );
    assert_eq!(
        parse("=kan !a.*b f00", &third, Some(&words)).fresh(),
        [true, false, false]
    );
    // What is kept matches as it did.
    let block = "漢字\nかん\n";
    assert_eq!(
        lines_of(&parse("kan", &third, Some(&words)), block),
        ["漢字", "かん"]
    );
}

#[test]
fn a_romaji_component_highlights_where_its_kana_match() {
    let query = LineQuery::parse("kan");
    let mut out = Vec::new();
    query.highlights("x かんじ KAN", &mut out);
    assert_eq!(out, [2..8, 12..15]);
}

/// The word of `family` (`漢`, `桜` or `田`) numbered `n` in [`heavy`].
fn heavy_word(family: char, n: usize) -> String {
    format!("{}{n:04}", family.to_string().repeat(18))
}

/// A dictionary in which the readings `ka`, `sa` and `ta` start give 250 words of 58 bytes
/// each, numbered in the order a pattern takes them: two of them spend most of one pattern's
/// budget of words.
fn heavy() -> Table {
    let kana: Vec<char> = ('ぁ'..='ゖ').collect();
    let mut text = String::new();
    for (first, family) in [('か', '漢'), ('さ', '桜'), ('た', '田')] {
        for n in 0..250 {
            let reading = format!("{first}{}{}", kana[n / 86], kana[n % 86]);
            text.push_str(&format!("{reading}\t{}\n", heavy_word(family, n)));
        }
    }
    dictionary(&text)
}

#[test]
fn an_inputs_migemo_components_share_one_patterns_words() {
    let table = heavy();
    let parse =
        |input: &str, before: &LineQuery| LineQuery::parse_after(input, before, true, Some(&table));
    let matches = |query: &LineQuery, line: &str| query.matches_line(line.as_bytes()).is_some();
    let line = |n: usize| {
        let (ka, sa) = (heavy_word('漢', 0), heavy_word('桜', 0));
        format!("{ka} {sa} {}", heavy_word('田', n))
    };
    let three = parse("ka sa ta", &LineQuery::empty());
    assert!(matches(&three, &line(0)));
    // The third has what the first two left: its first words, and its kana all the same.
    assert!(!matches(&three, &line(249)));
    let kana = format!("{} {} たぁ", heavy_word('漢', 0), heavy_word('桜', 0));
    assert!(matches(&three, &kana));
    // Alone, or first, it has them all.
    assert!(matches(
        &parse("ta", &LineQuery::empty()),
        &heavy_word('田', 249)
    ));
    assert!(matches(&parse("ta ka sa", &LineQuery::empty()), &line(249)));
    // A component kept spends what it did; one given more by a change before it compiles
    // again, and then has them.
    let more = parse("ka sa ta =x", &three);
    assert_eq!(more.fresh(), [false, false, false, true]);
    let two = parse("sa ta", &three);
    assert_eq!(two.fresh(), [false, true]);
    let pair = format!("{} {}", heavy_word('桜', 0), heavy_word('田', 249));
    assert!(matches(&two, &pair));
    // Back again, the third is given what it was given before, but by none kept from `two`.
    let again = parse("ka sa ta", &two);
    assert_eq!(again.fresh(), [true, false, true]);
    assert!(!matches(&again, &line(249)));
}

/// Lines that hold [`heavy`]'s words, early and late, and their kana.
fn heavy_lines() -> Vec<String> {
    let mut lines = Vec::new();
    for family in ['漢', '桜', '田'] {
        for n in [0, 63, 64, 120, 249] {
            lines.push(heavy_word(family, n));
        }
    }
    lines.extend(["かぁ", "さぁ", "たぁ", "ka", "KA", "x1"].map(String::from));
    lines
}

/// A small dictionary for the narrowing property: words in kanji, kana and letters.
fn small() -> Table {
    dictionary("かんじ\t漢字\nかな\t仮名\tカナ\nずつう\t頭痛\nつづき\t続き\nなまえ\t名前\tName\n")
}

/// Pieces a line for the narrowing property is made of.
const PIECES: &[&str] = &[
    "か", "ん", "な", "じ", "ず", "づ", "ま", "え", "カ", "ン", "ナ", "ズ", "ｶ", "ﾝ", "ｽﾞ", "ﾂﾞ",
    "漢字", "仮名", "頭痛", "名前", "k", "a", "n", "z", "u", "j", "i", "m", "e", "N", "ｋ", " ",
];

proptest! {
    /// A component kept from the input before, or from earlier in the same input, matches as
    /// one compiled for this input alone (ADR 0116, Q7).
    #[test]
    fn a_kept_component_matches_as_one_compiled_afresh(
        first in prop::collection::vec(0usize..9, 0..5),
        second in prop::collection::vec(0usize..9, 0..5),
    ) {
        const TOKENS: [&str; 9] = ["ka", "sa", "ta", "kan", "t", "=ka", "!sa", "x1", "Ta"];
        let table = heavy();
        let input = |picks: &[usize]| picks.iter().map(|&at| TOKENS[at]).collect::<Vec<_>>().join(" ");
        let before = LineQuery::parse_after(&input(&first), &LineQuery::empty(), true, Some(&table));
        let kept = LineQuery::parse_after(&input(&second), &before, true, Some(&table));
        let fresh = LineQuery::parse_after(&input(&second), &LineQuery::empty(), true, Some(&table));
        prop_assert_eq!(kept.components.len(), fresh.components.len());
        for (kept, fresh) in kept.components.iter().zip(&fresh.components) {
            prop_assert!(kept.spent == fresh.spent && kept.kind == fresh.kind);
            for line in heavy_lines() {
                prop_assert_eq!(
                    kept.regex.is_match(line.as_bytes()),
                    fresh.regex.is_match(line.as_bytes()),
                    "{:?} {:?}", input(&second), line
                );
            }
        }
    }

    /// Whenever a query narrows the one before, a search that ran, every line it matches is
    /// one that one matched, with and without a dictionary (ADR 0116, Q8).
    #[test]
    fn a_query_that_narrows_matches_no_line_the_one_before_did_not(
        old in "[kanszujimetKN' -]{1,6}",
        added in "[kanszujimeN ]{0,3}",
        prefix in "[kanszujime ]{0,3}",
        other in "[kanszujime ]{0,6}",
        lines in prop::collection::vec(prop::collection::vec(0usize..PIECES.len(), 0..6), 1..24),
        with_words: bool,
    ) {
        let table = small();
        let table = with_words.then_some(&table);
        let lines: Vec<String> = (lines.iter())
            .map(|pieces| pieces.iter().map(|&at| PIECES[at]).collect())
            .collect();
        let before = LineQuery::parse_after(&old, &LineQuery::empty(), true, table);
        if !before.starts() {
            return Ok(());
        }
        for new in [format!("{old}{added}"), format!("{prefix}{old}{added}"), other] {
            for query in [
                LineQuery::parse_after(&new, &before, true, table),
                LineQuery::parse_after(&new, &LineQuery::empty(), true, table),
            ] {
                if !query.narrows(&before) {
                    continue;
                }
                for line in &lines {
                    if query.matches_line(line.as_bytes()).is_some() {
                        prop_assert!(
                            before.matches_line(line.as_bytes()).is_some(),
                            "{:?} narrows {:?} but matches {:?}", new, old, line
                        );
                    }
                }
            }
        }
    }
}
