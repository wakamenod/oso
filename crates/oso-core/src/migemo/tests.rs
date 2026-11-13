//! Tests of migemo's patterns (ADR 0116): each row of the table, the rules between
//! spellings, unfinished ends, the forms, strings that are not romaji, the trie, and the
//! bound on a pattern's size.
//!
//! Must not depend on: any other module of this crate.

use proptest::prelude::*;
use regex::{Regex, RegexBuilder};

use super::{Romaji, is_romaji, pattern, spellings};

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

/// The first of each kana `string` gives, its second written after a `|`: `ず|づ`.
fn hiragana(string: &str) -> Option<String> {
    let romaji = Romaji::new(string)?;
    let mut out = String::new();
    for (c, other) in romaji.hiragana {
        out.push(c);
        if let Some(other) = other {
            out.push('|');
            out.push(other);
        }
    }
    Some(out)
}

#[test]
fn no_spelling_starts_another_and_each_is_its_row() {
    for (spelling, kana) in spellings::ROWS {
        for (other, _) in spellings::ROWS {
            assert!(
                spelling == other || !other.starts_with(spelling),
                "{spelling:?} starts {other:?}"
            );
        }
        let romaji = Romaji::new(spelling).unwrap();
        assert!(romaji.ends.is_empty(), "{spelling:?}");
        let first: String = romaji.hiragana.iter().map(|(c, _)| c).collect();
        assert_eq!(first, *kana);
        matches_whole(spelling, &[kana]);
    }
}

#[test]
fn a_spelling_that_gives_two_kana_finds_both_in_every_form() {
    for (spelling, other) in spellings::ALSO {
        let row = spellings::ROWS.iter().find(|(row, _)| row == spelling);
        let (_, kana) = row.unwrap_or_else(|| panic!("{spelling:?} has no row"));
        assert_eq!((kana.chars().count(), other.chars().count()), (1, 1));
        assert_eq!(hiragana(spelling), Some(format!("{kana}|{other}")));
        matches_whole(spelling, &[kana, other]);
    }
    matches_whole("zu", &["ず", "づ", "ズ", "ヅ", "ｽﾞ", "ﾂﾞ"]);
    matches_whole("ji", &["じ", "ぢ", "ジ", "ヂ", "ｼﾞ", "ﾁﾞ"]);
    matches_whole("tsuzuku", &["つづく", "つずく", "ツヅク", "ﾂﾂﾞｸ"]);
    matches_whole("hanaji", &["はなぢ", "はなじ", "ハナヂ"]);
    matches_whole("chijimu", &["ちぢむ"]);
    // An unfinished end takes them too.
    matches_whole("tsuz", &["つづ", "つず", "つざ"]);
    matches_whole("hanaj", &["はなぢ", "はなじゃ"]);
    // `ja` and `du` are not ones.
    assert!(!compiled("ja").is_match("ぢゃ"));
    assert!(!compiled("du").is_match("ず"));
}

#[test]
fn hepburns_m_before_b_and_p_is_n() {
    assert_eq!(hiragana("shimbunsha").as_deref(), Some("しんぶんしゃ"));
    assert_eq!(hiragana("tempura").as_deref(), Some("てんぷら"));
    matches_whole("shimbun", &["しんぶん", "シンブン", "ｼﾝﾌﾞﾝ"]);
    // Before `m` it is a doubled consonant, as input methods have it.
    assert_eq!(hiragana("semmei").as_deref(), Some("せっめい"));
    matches_whole("mma", &["っま", "ッマ"]);
    assert!(!compiled("mma").is_match("んま"));
    // At the end an `m` starts what it always did.
    let regex = compiled("shim");
    assert!(regex.is_match("しま") && regex.is_match("しみゃ"));
    assert!(!regex.is_match("しん"));
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
        ("kanji", "かんじ|ぢ"),
        ("shinbunsha", "しんぶんしゃ"),
        ("konnichiha", "こんいちは"),
        ("konnnichiha", "こんにちは"),
        ("kan'i", "かんい"),
        ("wo", "を"),
        ("ra-menn", "らーめん"),
        ("kwa", "くぁ"),
        ("gwa", "ぐぁ"),
        ("xyi", "ぃ"),
        ("lye", "ぇ"),
        ("tsuzuku", "つず|づく"),
        ("zizi", "じ|ぢじ|ぢ"),
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

    /// However long or odd a romaji string, its pattern stays within about 18 bytes a letter,
    /// compiles within a key's limits, and matches the string and its hiragana.
    #[test]
    fn any_romaji_gives_a_bounded_pattern(string in "[a-z'-]{1,256}") {
        let Some(text) = pattern(&string) else {
            return Ok(());
        };
        prop_assert!(text.len() <= 18 * string.len() + 2048, "{} bytes", text.len());
        let regex = compiled(&string);
        prop_assert!(regex.is_match(&string));
        let romaji = Romaji::new(&string).unwrap();
        let end = romaji.ends.first().copied().unwrap_or_default();
        let first: String = romaji.hiragana.iter().map(|(c, _)| c).collect();
        let second: String = romaji.hiragana.iter().map(|(c, other)| other.unwrap_or(*c)).collect();
        for kana in [first + end, second + end] {
            prop_assert!(regex.is_match(&kana));
        }
    }
}

#[test]
fn a_kana_and_a_combining_sound_mark_compose_as_unicode_has_it() {
    let (voiced, semi) = ('\u{3099}', '\u{309A}');
    for (base, mark, one) in [
        ('か', voiced, 'が'),
        ('う', voiced, 'ゔ'),
        ('ゝ', voiced, 'ゞ'),
        ('は', semi, 'ぱ'),
        ('ホ', semi, 'ポ'),
        ('ワ', voiced, 'ヷ'),
        ('ヽ', voiced, 'ヾ'),
    ] {
        assert_eq!(super::composed(base, mark), Some(one), "{base}");
    }
    for (base, mark) in [
        ('あ', voiced),
        ('か', semi),
        ('ん', voiced),
        ('a', voiced),
        ('が', voiced),
    ] {
        assert_eq!(super::composed(base, mark), None, "{base}");
    }
    assert_eq!(super::composed('か', 'x'), None);
    // Every voiced kana composes from the one before it, and every semi-voiced from the one
    // two before, but for the few Unicode gives apart.
    let pairs = ('\u{3041}'..='\u{30FF}')
        .flat_map(|base| [voiced, semi].map(|mark| (base, mark)))
        .filter_map(|(base, mark)| super::composed(base, mark).map(|one| (base, mark, one)));
    let mut count = 0;
    for (base, mark, one) in pairs {
        count += 1;
        let step = if mark == voiced { 1 } else { 2 };
        let apart = ['う', 'ゝ', 'ウ', 'ワ', 'ヰ', 'ヱ', 'ヲ', 'ヽ'];
        if !apart.contains(&base) {
            assert_eq!(u32::from(one), u32::from(base) + step, "{base}");
        }
    }
    assert_eq!(count, 58);
}
