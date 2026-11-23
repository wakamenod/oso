//! The list of what oso added: its file, its uses, and what was let go.
//!
//! Must not depend on: TypeWhisper, files or the app.

use super::*;

const DAY: u32 = 20_000;

fn often(wrong: &str, right: &str) -> Pair {
    Pair {
        wrong: wrong.into(),
        right: right.into(),
        fixes: 3,
        days: vec![DAY - 1, DAY],
        last: DAY,
        ..Pair::default()
    }
}

fn entry(wrong: &str, right: &str, kind: Kind) -> Entry {
    Entry {
        kind,
        wrong: wrong.into(),
        right: right.into(),
        unsure: false,
        added: DAY,
        used: DAY,
        state: State::Live,
        server: "here".into(),
    }
}

fn two() -> List {
    List {
        entries: vec![
            entry("リアクト", "React", Kind::Term),
            entry("機能", "昨日", Kind::Correction),
        ],
        let_go: Vec::new(),
    }
}

fn may_be_added(list: &List, pair: &Pair) -> bool {
    let record = Record {
        pairs: vec![pair.clone()],
    };
    !list.candidates(&record, "here").is_empty()
}

#[test]
fn the_list_is_read_back_whole_or_not_at_all() {
    let mut list = two();
    list.entries[1].unsure = true;
    list.note_let_go(&entry("a", "b", Kind::Correction), By::Person, DAY);
    list.note_let_go(&entry("c", "d", Kind::Term), By::Oso, DAY);
    let text = list.to_text();
    assert_eq!(List::parse(text.as_bytes(), DAY), Some(list.clone()));
    let earlier = List::parse(text.as_bytes(), DAY - 5).unwrap();
    assert_eq!(earlier.entries[0].used, DAY - 5, "no day after today");
    for broken in [
        "{",
        r#"{"version":2,"entries":[],"let_go":[]}"#,
        r#"{"version":1,"entries":[{}],"let_go":[]}"#,
        r#"{"version":1,"entries":[]}"#,
        &text.replace("live", "other"),
        &text.replace("React", &"長".repeat(41)),
    ] {
        assert_eq!(List::parse(broken.as_bytes(), DAY), None, "{broken}");
    }
    list.entries.push(list.entries[0].clone());
    assert_eq!(List::parse(list.to_text().as_bytes(), DAY), None, "twice");
    let many = List {
        entries: (0..=MAX_LISTED)
            .map(|n| entry(&format!("語{n}"), "x", Kind::Correction))
            .collect(),
        let_go: Vec::new(),
    };
    assert_eq!(List::parse(many.to_text().as_bytes(), DAY), None);
}

#[test]
fn a_dictation_marks_what_it_used() {
    let mut list = two();
    for entry in &mut list.entries {
        entry.used = DAY - 10;
    }
    assert!(
        !list.note_used("昨日の話", Some("昨日の話"), DAY),
        "said so"
    );
    assert!(list.note_used("昨日の話", Some("機能の話"), DAY));
    assert_eq!(list.entries[1].used, DAY);
    assert!(list.note_used("reactで", Some("reactで"), DAY));
    assert!(!list.note_used("reactで", None, DAY), "already today");
}

#[test]
fn what_the_person_let_go_never_comes_back_not_even_by_another_pair() {
    let mut list = List::default();
    list.note_let_go(&entry("リアクト", "React", Kind::Term), By::Person, DAY);
    list.note_let_go(&entry("機能", "昨日", Kind::Correction), By::Person, DAY);
    for pair in [
        often("リアクト", "React"),
        often("りあくと", "react"),
        often("機能", "帰納"),
    ] {
        assert!(!may_be_added(&list, &pair), "{pair:?}");
    }
    assert!(may_be_added(&list, &often("ビート", "Vite")));
    list.age(DAY + 400);
    assert_eq!(list.let_go.len(), 2, "until it is forgotten");
    list.forget();
    assert!(list.is_empty());
}

#[test]
fn what_oso_let_go_comes_back_once_seen_again_and_never_makes_the_persons_go() {
    let mut list = List::default();
    list.note_let_go(&entry("機能", "昨日", Kind::Correction), By::Oso, DAY);
    let mut pair = often("機能", "昨日");
    assert!(!may_be_added(&list, &pair));
    pair.last = DAY + 1;
    assert!(may_be_added(&list, &pair));
    list.note_let_go(&entry("ビート", "Vite", Kind::Term), By::Person, DAY);
    for n in 0..300 {
        list.note_let_go(
            &entry(&format!("語{n}語"), "x", Kind::Correction),
            By::Oso,
            DAY,
        );
    }
    assert_eq!(list.let_go.len(), MAX_LET_GO);
    assert!(!may_be_added(&list, &often("ビート", "Vite")));
}
