//! The tests of what dictation learns: the record's caps and age, the history and its caps,
//! what a watch's end teaches, and the log (ADR 0175, step 3).
//!
//! Must not depend on: files or the app; these run on strings and days alone.

use super::*;

fn seen(wrong: &str, right: &str, source: Source, outcome: Outcome) -> Observation {
    Observation {
        wrong: wrong.to_owned(),
        right: right.to_owned(),
        source,
        outcome,
    }
}

fn stood(wrong: &str, right: &str) -> Observation {
    seen(wrong, right, Source::Fix, Outcome::Stood)
}

#[test]
fn a_pair_counts_its_sources_its_undos_and_its_days() {
    let mut record = Record::default();
    record.observe(&[stood("リアクト", "React")], 100);
    record.observe(&[stood("リアクト", "React")], 100);
    record.observe(
        &[seen("リアクト", "React", Source::Edit, Outcome::Stood)],
        102,
    );
    record.observe(
        &[seen("リアクト", "React", Source::Fix, Outcome::Undone)],
        103,
    );
    let pair = &record.pairs[0];
    assert_eq!(record.pairs.len(), 1);
    assert_eq!((pair.fixes, pair.edits, pair.undone), (2, 1, 1));
    assert_eq!(
        pair.days,
        [100, 102],
        "undone days are not days it stood on"
    );
    assert_eq!(pair.last, 103);
    assert_eq!(pair.stood(), 3);
}

#[test]
fn a_pair_keeps_its_latest_days_only() {
    let mut record = Record::default();
    for day in 0..20 {
        record.observe(&[stood("あ", "a")], day);
    }
    let days = &record.pairs[0].days;
    assert_eq!(days.len(), MAX_DAYS);
    assert_eq!(days.first(), Some(&(20 - MAX_DAYS as u32)));
}

#[test]
fn a_pair_unseen_for_thirty_days_goes() {
    let mut record = Record::default();
    record.observe(&[stood("old", "new")], 10);
    record.observe(&[stood("keep", "kept")], 20);
    record.age(10 + UNUSED_DAYS - 1);
    assert_eq!(record.pairs.len(), 2);
    record.age(10 + UNUSED_DAYS);
    assert_eq!(record.pairs.len(), 1);
    assert_eq!(record.pairs[0].wrong, "keep");
}

#[test]
fn a_full_record_drops_the_pair_seen_longest_ago_and_least_used() {
    let mut record = Record::default();
    for n in 0..MAX_PAIRS {
        let day = 100 + u32::try_from(n.min(2)).unwrap();
        record.observe(&[stood(&format!("w{n}"), &format!("r{n}"))], day);
    }
    // w0 is used again, so w1 is now the one seen longest ago.
    record.observe(&[stood("w0", "r0")], 110);
    record.observe(&[stood("new", "pair")], 110);
    assert_eq!(record.pairs.len(), MAX_PAIRS);
    assert!(record.pairs.iter().all(|pair| pair.wrong != "w1"));
    assert!(record.pairs.iter().any(|pair| pair.wrong == "w0"));
    assert!(record.pairs.iter().any(|pair| pair.wrong == "new"));
}

#[test]
fn what_does_not_hold_is_never_kept() {
    let mut record = Record::default();
    let long = "a".repeat(fix::MAX_FIX_CHARS + 1);
    record.observe(
        &[
            stood("same", "same"),
            stood(" ", "x"),
            stood(&long, "x"),
            stood("x", "\u{202e}y"),
            stood("x", "a\u{7}"),
        ],
        1,
    );
    assert!(record.pairs.is_empty());
}

#[test]
fn the_record_reads_back_what_it_wrote_and_drops_what_does_not_hold() {
    let mut record = Record::default();
    record.observe(&[stood("リアクト", "React"), stood("ビート", "Vite")], 50);
    let text = record.to_text();
    assert_eq!(Record::parse(text.as_bytes(), 50), record);
    let crafted = r#"{"version":1,"pairs":[
        {"wrong":"a","right":"b","fixes":1,"edits":0,"plain":0,"undone":0,"days":[1],"last":1},
        {"wrong":"a","right":"b","fixes":9,"edits":0,"plain":0,"undone":0,"days":[1],"last":1},
        {"wrong":"c","right":"c","fixes":1,"edits":0,"plain":0,"undone":0,"days":[1],"last":1},
        {"wrong":"d","right":"e","fixes":-1,"edits":0,"plain":0,"undone":0,"days":[1],"last":1},
        {"wrong":"f","right":"g","fixes":1,"edits":0,"plain":0,"undone":0,"days":[1],"last":"x"},
        {"wrong":"h","right":"i","fixes":1,"edits":0,"plain":0,"undone":0,"days":[9,1,9,3],"last":9}
    ]}"#;
    let read = Record::parse(crafted.as_bytes(), 9);
    let wrongs: Vec<&str> = read.pairs.iter().map(|pair| pair.wrong.as_str()).collect();
    assert_eq!(wrongs, ["a", "h"]);
    assert_eq!(read.pairs[0].fixes, 1, "the first of a pair is kept");
    assert_eq!(read.pairs[1].days, [1, 3, 9]);
    assert_eq!(
        Record::parse(b"{\"version\":2,\"pairs\":[]}", 1),
        Record::default()
    );
    assert_eq!(Record::parse(b"not json", 1), Record::default());
}

#[test]
fn the_history_is_of_pairs_in_the_text_the_most_used_first() {
    let mut record = Record::default();
    record.observe(&[stood("often", "OFTEN")], 10);
    record.observe(&[stood("often", "OFTEN")], 11);
    record.observe(&[stood("latest", "LATEST")], 12);
    record.observe(&[stood("ここ", "here")], 5);
    record.observe(&[stood("elsewhere", "ELSEWHERE")], 12);
    let history = record.history("ここで latest と often を使う");
    let order: Vec<&str> = history
        .accepted
        .iter()
        .map(|(w, _, _)| w.as_str())
        .collect();
    assert_eq!(order, ["often", "latest", "ここ"], "none not in the text");
    assert_eq!(history.accepted[0].2, 2);
    assert!(history.rejected.is_empty());
    assert_eq!(record.history("nothing of them"), History::default());
}

#[test]
fn an_undone_pair_is_rejected_only_where_its_wrong_is_in_the_text() {
    let mut record = Record::default();
    record.observe(
        &[
            seen("ビュー", "view", Source::Fix, Outcome::Undone),
            seen("other", "OTHER", Source::Fix, Outcome::Undone),
            stood("tied", "TIED"),
            seen("tied", "TIED", Source::Fix, Outcome::Undone),
        ],
        3,
    );
    let history = record.history("ビューと tied");
    assert!(history.accepted.is_empty(), "undone as often as it stood");
    let rejected: Vec<&str> = history.rejected.iter().map(|(w, _)| w.as_str()).collect();
    assert_eq!(rejected, ["tied", "ビュー"]);
}

#[test]
fn the_history_keeps_to_its_pairs_and_bytes() {
    let mut record = Record::default();
    for n in 0..MAX_PAIRS {
        record.observe(&[stood(&format!("w{n:03}"), &format!("r{n:03}"))], 1);
    }
    let all: String = (0..MAX_PAIRS).map(|n| format!("w{n:03} ")).collect();
    assert!(record.history(&all).accepted.len() <= MAX_HISTORY);
    let mut big = Record::default();
    for n in 0..MAX_PAIRS {
        let wrong = format!("{n:02}{}", "あ".repeat(60));
        big.observe(&[stood(&wrong, "x")], 1);
    }
    let all: String = (0..MAX_PAIRS)
        .map(|n| format!("{n:02}{} ", "あ".repeat(60)))
        .collect();
    let history = big.history(&all);
    let bytes: usize = history
        .accepted
        .iter()
        .map(|(w, r, _)| w.len() + r.len())
        .sum();
    assert!(bytes <= MAX_HISTORY_BYTES);
    assert!(!history.accepted.is_empty());
}

fn watched(typed: &str, left: &str, fixed: &[(&str, &str)], now: &str) -> Watched {
    Watched {
        typed: typed.to_owned(),
        left: left.to_owned(),
        fixed: fixed
            .iter()
            .map(|(w, r)| ((*w).to_owned(), (*r).to_owned()))
            .collect(),
        current: now.to_owned(),
        sent: true,
    }
}

#[test]
fn a_fix_left_alone_stood_and_one_undone_was_undone() {
    let fixed = [("リアクト", "React")];
    let left = watched("リアクトで", "Reactで", &fixed, "Reactで");
    assert_eq!(observations(&left), [stood("リアクト", "React")]);
    let undone = watched("リアクトで", "Reactで", &fixed, "リアクトで");
    assert_eq!(
        observations(&undone),
        [seen("リアクト", "React", Source::Fix, Outcome::Undone)]
    );
}

#[test]
fn the_persons_rewrite_is_learned_beside_the_fix_it_left() {
    let fixed = [("リアクト", "React")];
    let edited = watched(
        "リアクトとビートで",
        "Reactとビートで",
        &fixed,
        "ReactとViteで",
    );
    assert_eq!(
        observations(&edited),
        [
            stood("リアクト", "React"),
            seen("ビート", "Vite", Source::Edit, Outcome::Stood),
        ]
    );
}

#[test]
fn a_fix_the_person_put_back_by_hand_was_undone_and_is_no_rewrite() {
    let fixed = [("リアクト", "React"), ("ビート", "Vite")];
    let back = watched(
        "リアクトとビートで",
        "ReactとViteで",
        &fixed,
        "リアクトとViteで",
    );
    assert_eq!(
        observations(&back),
        [
            seen("リアクト", "React", Source::Fix, Outcome::Undone),
            stood("ビート", "Vite"),
        ]
    );
}

#[test]
fn a_plain_dictation_left_alone_teaches_nothing() {
    let plain = watched("あいう", "あいう", &[], "あいう");
    assert!(observations(&plain).is_empty());
    let gone = watched("リアクト", "React", &[("リアクト", "React")], "");
    assert!(observations(&gone).is_empty());
}

#[test]
fn the_log_keeps_its_last_lines() {
    let at = 100 * 86_400;
    let lines: Vec<String> = (0..MAX_LOG_LINES + 10)
        .map(|n| format!("{{\"at\":{at},\"n\":{n}}}"))
        .collect();
    let log = append_log("old\n\n", &lines, at);
    let kept: Vec<&str> = log.lines().collect();
    assert_eq!(kept.len(), MAX_LOG_LINES);
    assert!(kept[0].contains("\"n\":10"));
    let month = u64::from(UNUSED_DAYS) * 86_400;
    let old = format!("{{\"at\":{}}}\n{{\"at\":{}}}\n", at - month - 1, at - month);
    assert_eq!(
        append_log(&old, &["new".to_owned()], at),
        format!("{{\"at\":{}}}\nnew\n", at - month),
        "a line from past the month goes"
    );
    let line = log_observed(&stood("w", "r"), 7);
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["source"], "fix");
    assert_eq!(value["outcome"], "stood");
    let counts = log_history(
        &History {
            accepted: vec![("a".into(), "b".into(), 1)],
            rejected: Vec::new(),
        },
        7,
    );
    assert!(!counts.contains("\"a\""), "the history is logged as counts");
}

#[test]
fn a_day_is_counted_from_the_epoch() {
    assert_eq!(day_of(0), 0);
    assert_eq!(day_of(86_399), 0);
    assert_eq!(day_of(86_400), 1);
}

#[test]
fn a_pair_that_looks_like_a_credential_is_never_kept() {
    for right in [
        "ghp_abc",
        "github_pat_11AB",
        "sk-proj",
        "xoxb-1234",
        "AKIAIOSFODNN7EXAMPLE",
        "a1b2c3d4e5f6g7h8i9",
        "-----BEGIN",
    ] {
        assert!(looks_like_a_credential(right), "{right}");
        let mut record = Record::default();
        record.observe(&[seen("トークン", right, Source::Edit, Outcome::Stood)], 1);
        assert!(record.pairs.is_empty(), "{right}");
    }
    for word in [
        "useEffect",
        "task-runner",
        "ASIA",
        "React18",
        "v1.2.3",
        "トークン",
    ] {
        assert!(!looks_like_a_credential(word), "{word}");
    }
}

#[test]
fn a_day_past_today_in_the_record_is_today() {
    let crafted = r#"{"version":1,"pairs":[
        {"wrong":"a","right":"b","fixes":1,"edits":0,"plain":0,"undone":0,"days":[50,900],"last":900}
    ]}"#;
    let read = Record::parse(crafted.as_bytes(), 60);
    assert_eq!(read.pairs[0].last, 60);
    assert_eq!(read.pairs[0].days, [50, 60]);
    let mut aged = read.clone();
    aged.age(60 + UNUSED_DAYS);
    assert!(aged.pairs.is_empty(), "it ages as any");
}

#[test]
fn a_rewrite_of_text_typed_plain_is_kept_but_never_sent() {
    let mut plain = watched("ビートで", "ビートで", &[], "Viteで");
    plain.sent = false;
    let seen = observations(&plain);
    assert_eq!(seen, [seen_as("ビート", "Vite", Source::Plain)]);
    let mut record = Record::default();
    record.observe(&seen, 4);
    assert_eq!((record.pairs[0].plain, record.pairs[0].stood()), (1, 1));
    assert_eq!(
        record.history("ビート"),
        History::default(),
        "kept from Claude"
    );
    let text = record.to_text();
    assert_eq!(Record::parse(text.as_bytes(), 4), record);
    // Once also learned from text a fix saw, it is sent, counted by that alone.
    record.observe(&[seen_as("ビート", "Vite", Source::Edit)], 5);
    assert_eq!(
        record.history("ビート").accepted,
        [("ビート".to_owned(), "Vite".to_owned(), 1)]
    );
}

fn seen_as(wrong: &str, right: &str, source: Source) -> Observation {
    seen(wrong, right, source, Outcome::Stood)
}
