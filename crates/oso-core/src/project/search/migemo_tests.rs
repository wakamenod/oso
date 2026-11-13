//! Tests of a project search's migemo components that share one input's budget (ADR 0116,
//! Q7) and of narrowing by their strings (Q8): words spent in input order, lookup steps run
//! out, components kept as they would compile, and properties over generated input.
//!
//! Must not depend on: `editor`, threads or the file system.

use proptest::prelude::*;

use super::LineQuery;
use super::tests::dictionary;
use crate::migemo::Table;

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

#[test]
fn a_component_whose_lookup_ran_out_of_steps_is_kept_only_as_it_would_compile() {
    // Readings of `ず` or `づ` 11 times, 10 `あ` and `か`: each lookup through them takes more
    // than half of an input's steps, so a second runs out.
    let mut text = String::new();
    for n in 0..2048usize {
        let reading: String = (0..11)
            .map(|bit| if n >> bit & 1 == 1 { 'ず' } else { 'づ' })
            .chain(std::iter::repeat_n('あ', 10))
            .chain(['か'])
            .collect();
        text.push_str(&format!("{reading}\t語{n}\n"));
    }
    let table = dictionary(&text);
    let parse =
        |input: &str, before: &LineQuery| LineQuery::parse_after(input, before, true, Some(&table));
    let stem = format!("{}{}", "zu".repeat(11), "a".repeat(10));
    let (miss, hit) = (format!("{stem}ni"), format!("{stem}ka"));
    let both = parse(&format!("{miss} {hit}"), &LineQuery::empty());
    let kana = |end: char| format!("{}{}{end}", "ず".repeat(11), "あ".repeat(10));
    let matches = |query: &LineQuery, line: &str| query.matches_line(line.as_bytes()).is_some();
    let word = format!("{} 語7 x", kana('に'));
    assert!(matches(&both, &format!("{} {} x", kana('に'), kana('か'))));
    assert!(
        !matches(&both, &word),
        "out of steps, the second has its kana alone"
    );
    let again = parse(&format!("{miss} {hit} =x"), &both);
    assert_eq!(again.fresh(), [false, false, true]);
    assert!(!matches(&again, &word));
    // Alone, it has the steps, so it compiles again and finds its words.
    let alone = parse(&hit, &both);
    assert_eq!(alone.fresh(), [true]);
    assert!(matches(&alone, &word));
}

#[test]
fn a_component_given_fewer_words_before_does_not_narrow_to_the_same_text() {
    let table = heavy();
    let parse =
        |input: &str, before: &LineQuery| LineQuery::parse_after(input, before, true, Some(&table));
    let before = parse("ka sa ta", &LineQuery::empty());
    // The literals spend no words, so `ta` has them all now.
    let after = parse("ka1 sa1 ta", &before);
    let line = format!("ka1 sa1 {}", heavy_word('田', 249));
    assert!(after.matches_line(line.as_bytes()).is_some());
    assert!(before.matches_line(line.as_bytes()).is_none());
    assert!(!after.narrows(&before));
}

#[test]
fn a_component_with_another_dictionarys_words_does_not_narrow_to_the_same_text() {
    // The same readings and as many bytes of words, so it spends the same.
    let one = dictionary("かんじ\t漢字\n");
    let other = Table {
        generation: 2,
        ..dictionary("かんじ\t感じ\n")
    };
    let before = LineQuery::parse_after("kanji", &LineQuery::empty(), true, Some(&one));
    let after = LineQuery::parse_after("kanji", &before, true, Some(&other));
    assert!(after.matches_line("感じ".as_bytes()).is_some());
    assert!(before.matches_line("感じ".as_bytes()).is_none());
    assert!(!after.narrows(&before));
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

/// The dictionaries a search may have had for the narrowing property: none, a small one with
/// words in kanji, kana and letters, the same in a new generation, and another one.
fn tables() -> [Option<Table>; 4] {
    let small = "かんじ\t漢字\nかな\t仮名\tカナ\nずつう\t頭痛\nつづき\t続き\nなまえ\t名前\tName\n";
    let again = Table {
        generation: 2,
        ..dictionary(small)
    };
    let other = Table {
        generation: 3,
        ..dictionary("かんじ\t感じ\tkanji\nかな\t金\nずつう\t図\nなまえ\tnamae\tName\n")
    };
    [None, Some(dictionary(small)), Some(again), Some(other)]
}

/// Pieces a line for the narrowing property is made of.
const PIECES: &[&str] = &[
    "か", "ん", "な", "じ", "ず", "づ", "ま", "え", "カ", "ン", "ナ", "ズ", "ｶ", "ﾝ", "ｽﾞ", "ﾂﾞ",
    "漢字", "仮名", "頭痛", "名前", "感じ", "金", "図", "k", "a", "n", "z", "u", "j", "i", "m",
    "e", "N", "ｋ", " ", "=", "!", "Ｋ", "\u{212a}", "ſ", "İ", "i\u{307}",
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
    /// one that one matched (ADR 0116, Q8): with and without a dictionary, with the same
    /// settings or as the setting flips and a dictionary comes, goes or is another, so that a
    /// component is a literal before and migemo's after or the other way, and with `=` and
    /// `!` at a component's start.
    #[test]
    fn a_query_that_narrows_matches_no_line_the_one_before_did_not(
        old in "[kanszujimetKN'=! -]{1,6}",
        added in "[kanszujimeNKİſ\u{212a} ]{0,3}",
        prefix in "[kanszujime=! ]{0,3}",
        other in "[kanszujimeKİ\u{212a}ſ=! ]{0,6}",
        lines in prop::collection::vec(prop::collection::vec(0usize..PIECES.len(), 0..6), 1..24),
        settings in (any::<bool>(), 0usize..4, any::<bool>(), 0usize..4),
    ) {
        let tables = tables();
        let (old_migemo, old_table, new_migemo, new_table) = settings;
        let lines: Vec<String> = (lines.iter())
            .map(|pieces| pieces.iter().map(|&at| PIECES[at]).collect())
            .collect();
        let empty = LineQuery::empty();
        let before = LineQuery::parse_after(&old, &empty, old_migemo, tables[old_table].as_ref());
        if !before.starts() {
            return Ok(());
        }
        for new in [format!("{old}{added}"), format!("{prefix}{old}{added}"), other] {
            for (migemo, table) in [(old_migemo, old_table), (new_migemo, new_table)] {
                let table = tables[table].as_ref();
                for query in [
                    LineQuery::parse_after(&new, &before, migemo, table),
                    LineQuery::parse_after(&new, &empty, migemo, table),
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
}
