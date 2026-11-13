//! Tests of the `%` affix (ADR 0116): a romaji component matches migemo's pattern, with a
//! dictionary's words once one is given, over a candidate's kana composed, and anything else
//! after `%` is a literal.
//!
//! Must not depend on: `editor` or `minibuffer`; these run on strings alone.

use std::ops::Range;
use std::sync::Arc;

use proptest::prelude::*;

use super::{Candidate, Candidates, Query};
use crate::migemo::{Dictionary, Table};

fn candidate(text: &str) -> Candidate {
    Candidate::new(text, "", "")
}

fn words() -> Table {
    Table {
        generation: 1,
        dictionary: Arc::new(Dictionary::new("かんじ\t漢字\n".to_owned())),
    }
}

fn query(input: &str, dictionary: Option<&Table>) -> Query {
    Query::parse_after(input, &Query::default(), dictionary)
}

fn matches(input: &str, text: &str) -> bool {
    Query::parse(input).matches(&candidate(text))
}

fn highlights(input: &str, text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    Query::parse(input).highlights(&candidate(text), &mut out);
    out
}

#[test]
fn percent_makes_romaji_match_its_kana_and_itself() {
    for input in ["%kan", "kan%"] {
        for text in [
            "かんじ.md",
            "カンジ.md",
            "ｶﾝｼﾞ",
            "kanji.md",
            "KANJI.md",
            "ｋａｎ",
        ] {
            assert!(matches(input, text), "{input:?} {text:?}");
        }
        assert!(!matches(input, "漢字.md"), "no dictionary, no kanji");
        assert!(!matches(input, "other.md"));
    }
    // Without `%`, a component matches as ADR 0076 says.
    assert!(!matches("kan", "かんじ.md"));
    assert!(matches("kan", "kanji.md"));
    let query = Query::parse("%kan");
    assert!(query.uses_migemo() && query.wants_dictionary());
    assert!(!Query::parse("kan").uses_migemo());
}

#[test]
fn a_dictionary_adds_its_words() {
    let words = words();
    let with = query("%kan", Some(&words));
    assert!(with.matches(&candidate("漢字.md")));
    assert!(with.matches(&candidate("かんじ.md")));
    assert!(!with.wants_dictionary());
}

#[test]
fn percent_before_anything_but_romaji_is_a_literal() {
    assert!(matches("%a.b", "a.b"));
    assert!(!matches("%a.b", "axb"));
    assert!(matches("%struct", "struct.rs"));
    // An upper-case letter is not romaji, and makes case count.
    assert!(matches("%Kan", "Kanji"));
    assert!(!matches("%Kan", "kanji"));
    assert!(!matches("%Kan", "カンジ"));
    assert!(!Query::parse("%Kan").uses_migemo());
    // `%` alone matches everything, as every affix does.
    assert!(matches("%", "anything"));
}

#[test]
fn a_migemo_component_highlights_where_it_matched() {
    assert_eq!(highlights("%kan", "x/かんじ.md"), vec![2..8]);
    assert_eq!(highlights("%kan .md", "x/カンジ.md"), [2..8, 11..14]);
    assert_eq!(highlights("%kan", "KANJI"), vec![0..3]);
}

#[test]
fn decomposed_kana_are_matched_composed() {
    // As macOS names files: `が` as `か` and U+3099, `ぱ` as `は` and U+309A.
    let decomposed = "か\u{3099}っこう.md";
    assert!(matches("%gakkou", decomposed));
    assert!(!matches("%kakkou", decomposed), "a mark is not left off");
    // Where it matched is the original's, the mark with its kana.
    assert_eq!(highlights("%ga", decomposed), vec![0..6]);
    assert!(matches("%pan", "ハ\u{309A}ン"));
    assert!(matches("%pan", "は\u{309A}ん"));
    // A mark after a kana it does not compose with stays as it is.
    assert!(matches("%n", "ん\u{3099}"));
    assert_eq!(highlights("%n", "ん\u{3099}"), vec![0..3]);
    // A component without `%` reads the text as it is.
    assert!(matches("か\u{3099}", decomposed));
    assert!(!matches("が", decomposed));
}

#[test]
fn a_migemo_component_narrows_only_as_it_is_with_its_dictionary() {
    let words = words();
    assert!(!query("%kanj", None).narrows(&query("%kan", None)));
    assert!(query("%kan x", None).narrows(&query("%kan", None)));
    assert!(query("%kan", Some(&words)).narrows(&query("%kan", Some(&words))));
    assert!(!query("%kan", Some(&words)).narrows(&query("%kan", None)));
    assert!(!query("%kan", None).narrows(&query("%kan", Some(&words))));
    assert_ne!(query("%kan", Some(&words)), query("%kan", None));
}

#[test]
fn a_dictionary_that_comes_matches_the_input_again() {
    let items: Arc<[Candidate]> = ["漢字.md", "かんじ.md", "other"]
        .iter()
        .map(|text| candidate(text))
        .collect();
    let mut list = Candidates::new(items, []);
    list.set_input("%kan");
    assert_eq!(list.len(), 1);
    assert!(!list.stale_for(None), "the same: nothing to match again");
    let words = words();
    assert!(list.stale_for(Some(words.generation)));
    list.set_input_with("%kan", Some(&words));
    assert_eq!(list.len(), 2);
    assert!(!list.stale_for(Some(words.generation)));
    // And when it goes.
    assert!(list.stale_for(None));
    // An input with no migemo component need not be matched again.
    list.set_input("other");
    assert!(!list.stale_for(Some(2)));
}

#[test]
fn a_component_repeated_in_one_input_compiles_once_and_romaji_without_kana_is_a_literal() {
    let query = Query::parse("%kan %kan %struct");
    assert!(query.uses_migemo());
    assert!(!query.compiled_for_another(None));
    // `struct` gives no kana, so it is a literal, which no dictionary changes.
    assert!(!Query::parse("%struct").compiled_for_another(Some(1)));
}

proptest! {
    #[test]
    fn matching_agrees_with_finding_where(
        texts in prop::collection::vec("[kaんかはカハ\u{3099}\u{309A}ｶﾞ.-]{0,8}", 1..8),
        input in "[%kanhgp' -]{0,6}",
    ) {
        let query = Query::parse(&input);
        for text in &texts {
            let item = candidate(text);
            prop_assert_eq!(
                query.matches(&item),
                query.matches_finding_where(&item),
                "{:?} in {:?}",
                input,
                text
            );
            let mut out = Vec::new();
            query.highlights(&item, &mut out);
            for range in out {
                prop_assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            }
        }
    }
}
