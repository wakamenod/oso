//! Tests of migemo's patterns (ADR 0116): each row of the table, the rules between
//! spellings, unfinished ends, the forms, strings that are not romaji, the trie, and the
//! bound on a pattern's size.
//!
//! Must not depend on: any other module of this crate.

use proptest::prelude::*;
use regex::{Regex, RegexBuilder};

use super::{Romaji, is_romaji, pattern, table};

/// The pattern for `string`, compiled as isearch compiles it on a key.
fn compiled(string: &str) -> Regex {
    let pattern = pattern(string).unwrap_or_else(|| panic!("{string:?} gives no pattern"));
    RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .size_limit(256 * 1024)
        .nest_limit(32)
        .build()
        .unwrap()
}

/// Whether `string`'s pattern matches the whole of each of `texts`.
fn matches_whole(string: &str, texts: &[&str]) {
    let regex = compiled(string);
    for text in texts {
        let found = regex.find(text).map(|found| found.as_str());
        assert_eq!(found, Some(*text), "{string:?} on {text:?}");
    }
}

fn hiragana(string: &str) -> Option<String> {
    Romaji::new(string).map(|romaji| romaji.hiragana)
}

#[test]
fn no_spelling_starts_another_and_each_is_its_row() {
    for (spelling, kana) in table::ROWS {
        for (other, _) in table::ROWS {
            assert!(
                spelling == other || !other.starts_with(spelling),
                "{spelling:?} starts {other:?}"
            );
        }
        let romaji = Romaji::new(spelling).unwrap();
        assert_eq!((romaji.hiragana.as_str(), romaji.ends.len()), (*kana, 0));
        matches_whole(spelling, &[kana]);
    }
}

#[test]
fn spellings_of_each_system_and_the_rules_between_them() {
    for (string, kana) in [
        ("shi", "し"),
        ("si", "し"),
        ("chi", "ち"),
        ("ti", "ち"),
        ("tsu", "つ"),
        ("tu", "つ"),
        ("fu", "ふ"),
        ("hu", "ふ"),
        ("ja", "じゃ"),
        ("jya", "じゃ"),
        ("zya", "じゃ"),
        ("xa", "ぁ"),
        ("ltu", "っ"),
        ("xtsu", "っ"),
        ("kka", "っか"),
        ("matcha", "まっちゃ"),
        ("kanji", "かんじ"),
        ("shinbunsha", "しんぶんしゃ"),
        ("konnichiha", "こんいちは"),
        ("konnnichiha", "こんにちは"),
        ("kan'i", "かんい"),
        ("wo", "を"),
        ("ra-menn", "らーめん"),
        ("n-", "んー"),
    ] {
        assert_eq!(hiragana(string).as_deref(), Some(kana), "{string:?}");
    }
}

#[test]
fn an_unfinished_end_matches_every_kana_it_can_start() {
    matches_whole(
        "k",
        &["か", "き", "く", "け", "こ", "っ", "カ", "ｯ", "k", "K"],
    );
    matches_whole("ky", &["きゃ", "きゅ", "きょ", "キャ", "ｷｮ"]);
    matches_whole("kan", &["かん", "かな", "かにゃ", "カン", "ｶﾅ"]);
    matches_whole("kak", &["かか", "かっ", "カキ"]);
    matches_whole("mat", &["また", "まっ", "まつ"]);
    matches_whole("matc", &["まっち", "まっちゃ"]);
    // Only a kana the end can start.
    let regex = compiled("k");
    assert!(!regex.is_match("さ"));
    assert!(!regex.is_match("ん"));
}

#[test]
fn every_form_of_the_kana_and_the_letters() {
    matches_whole("ka", &["ka", "KA", "か", "カ", "ｶ", "ｋａ", "ＫＡ"]);
    matches_whole("ga", &["が", "ガ", "ｶﾞ"]);
    matches_whole("pa", &["ぱ", "パ", "ﾊﾟ"]);
    matches_whole("vu", &["ゔ", "ヴ", "ｳﾞ"]);
    matches_whole("ji", &["じ", "ジ", "ｼﾞ"]);
    matches_whole("du", &["づ", "ヅ", "ﾂﾞ"]);
    matches_whole(
        "ra-men",
        &["らーめん", "ラーメン", "ﾗｰﾒﾝ", "ra-men", "ｒａ－ｍｅｎ"],
    );
    matches_whole("xwa", &["ゎ", "ヮ", "ﾜ"]);
    let regex = compiled("ka");
    assert!(!regex.is_match("が"));
}

#[test]
fn a_string_that_is_not_romaji_gives_no_pattern() {
    for string in [
        "", "'", "-", "Ka", "ka ", "ka1", "かa", "struct", "rust", "http", "kxa",
    ] {
        assert_eq!(pattern(string), None, "{string:?}");
    }
    assert!(is_romaji("n'-"));
    assert!(!is_romaji("'-"));
    assert_eq!(pattern(&"a".repeat(super::MAX_CHARS + 1)), None);
}

#[test]
fn the_pattern_takes_common_prefixes_out_and_escapes_every_character() {
    assert_eq!(
        pattern("ky").as_deref(),
        Some("(?:ky|き(?:ぃ|ぇ|ゃ|ゅ|ょ)|キ(?:ィ|ェ|ャ|ュ|ョ)|ｋｙ|ｷ(?:ｨ|ｪ|ｬ|ｭ|ｮ))")
    );
    let dashed = pattern("a-").unwrap();
    assert!(dashed.starts_with("(?:a\\-|"), "{dashed}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// However long or odd a romaji string, its pattern stays within about 16 bytes a letter,
    /// compiles within a key's limits, and matches the string and its hiragana.
    #[test]
    fn any_romaji_gives_a_bounded_pattern(string in "[a-z'-]{1,256}") {
        let Some(text) = pattern(&string) else {
            return Ok(());
        };
        prop_assert!(text.len() <= 16 * string.len() + 2048, "{} bytes", text.len());
        let regex = compiled(&string);
        prop_assert!(regex.is_match(&string));
        let romaji = Romaji::new(&string).unwrap();
        let end = romaji.ends.first().copied().unwrap_or_default();
        let kana = format!("{}{end}", romaji.hiragana);
        prop_assert!(regex.is_match(&kana));
    }
}
