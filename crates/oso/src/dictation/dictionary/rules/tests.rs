//! Which learned pairs are promoted, and as what.
//!
//! Must not depend on: TypeWhisper, files or the app.

use super::*;
use crate::dictation::dictionary::{corrections, terms};

const DAY: u32 = 20_000;

fn pair(wrong: &str, right: &str, stood: u32, days: &[u32]) -> Pair {
    Pair {
        wrong: wrong.into(),
        right: right.into(),
        fixes: stood,
        days: days.to_vec(),
        last: DAY,
        ..Pair::default()
    }
}

fn often(wrong: &str, right: &str) -> Pair {
    pair(wrong, right, 3, &[DAY - 1, DAY])
}

/// What is promoted of `pairs`, with its kind.
fn promoted(pairs: &[Pair]) -> Vec<(String, Kind)> {
    let record = Record {
        pairs: pairs.to_vec(),
    };
    (record.pairs.iter())
        .filter(|pair| ready(pair) && unfit(pair, &record).is_none())
        .map(|pair| (pair.wrong.clone(), kind_of(pair)))
        .collect()
}

#[test]
fn a_pair_is_promoted_after_three_over_two_days_but_never_from_plain_text_alone() {
    assert!(promoted(&[pair("ビート", "Vite", 2, &[DAY - 1, DAY])]).is_empty());
    assert!(promoted(&[pair("ビート", "Vite", 9, &[DAY])]).is_empty());
    let mut undone = often("ビート", "Vite");
    undone.undone = 3;
    assert!(
        promoted(&[undone]).is_empty(),
        "undone as often as it stood"
    );
    let mut plain = pair("ビート", "Vite", 0, &[DAY - 1, DAY]);
    plain.plain = 30;
    assert!(promoted(&[plain.clone()]).is_empty(), "kept from Claude");
    plain.plain = 2;
    plain.edits = 1;
    assert_eq!(promoted(&[plain]), [("ビート".into(), Kind::Term)]);
}

#[test]
fn ascii_from_kana_is_a_term_and_anything_else_a_correction() {
    assert_eq!(
        promoted(&[
            often("リアクト", "React"),
            often("いしゅー", "issue"),
            often("機能", "昨日"),
            often("リ亜クト", "React"),
            often("ビート", "美糸"),
        ]),
        [
            ("リアクト".into(), Kind::Term),
            ("いしゅー".into(), Kind::Term),
            ("機能".into(), Kind::Correction),
            ("リ亜クト".into(), Kind::Correction),
            ("ビート".into(), Kind::Correction),
        ]
    );
}

#[test]
fn a_pair_whose_own_text_would_change_too_much_is_not_promoted() {
    for (wrong, right) in [
        ("機", "樹"),
        ("です", "でつ"),
        ("to", "too much"),
        ("vite", "Vite"),
        ("ビート", "ビートル"),
        ("ビート", ""),
        (" ビート", "Vite"),
        ("ビート", "Vite "),
        ("123", "one"),
        ("e\u{301}te", "ete"),
        ("機能", "strasse ß"),
    ] {
        assert!(
            promoted(&[often(wrong, right)]).is_empty(),
            "{wrong:?} → {right:?}"
        );
    }
    assert!(promoted(&[often(&"長".repeat(41), "短い")]).is_empty());
    assert!(
        promoted(&[often("機能", "昨日"), often("機能", "帰納")]).is_empty(),
        "two rights as often"
    );
}

#[test]
fn only_the_persons_own_rewrite_back_takes_a_pair_back() {
    let forward = often("機能", "昨日");
    let mut back = pair("昨日", "機能", 1, &[DAY]);
    assert_eq!(promoted(&[forward.clone(), back.clone()]).len(), 1, "a fix");
    back.edits = 1;
    let record = Record {
        pairs: vec![forward.clone(), back],
    };
    assert!(taken_back(&forward, &record, DAY));
    assert!(
        !taken_back(&forward, &record, DAY + 1),
        "before it was added"
    );
}

#[test]
fn a_clash_with_what_typewhisper_holds_is_seen_whatever_its_case_or_accents() {
    let dictionary = Dictionary {
        terms: terms(r#"{"terms":["Café","東京"]}"#.as_bytes()).unwrap(),
        corrections: corrections(
            "{\"corrections\":[\
             {\"original\":\"Ty\",\"replacement\":\"TypeWhisper\",\"caseSensitive\":false},\
             {\"original\":\"か\u{3099}ぞう\",\"replacement\":\"画像\",\"caseSensitive\":false}]}"
                .as_bytes(),
        )
        .unwrap(),
    };
    let clash =
        |wrong: &str, right: &str, kind| clashes((wrong, right), kind, &dictionary).is_some();
    assert!(clash("カフェ", "cafe", Kind::Term));
    assert!(!clash("エーアイ", "AI", Kind::Term), "東京 is another word");
    assert!(clash("tY", "Tie", Kind::Correction));
    assert!(
        clash("がぞう", "合造", Kind::Correction),
        "a mark of its own"
    );
    assert!(
        clash("whisper", "voice", Kind::Correction),
        "in a replacement"
    );
    assert!(
        clash("何か", "mighty", Kind::Correction),
        "holds an original"
    );
    assert!(!clash("機能", "昨日", Kind::Correction));
}
