//! Tests of stashes' pure parts: the list read into rows, selectors, a stash found again by its
//! id, a push's arguments, failures and the drop's question, with a property test over any
//! bytes.
//!
//! Must not depend on: git or the file system.

use proptest::prelude::*;

use super::{Act, args, drop_question, failed, find, is_selector, parse, push_args, title};

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn record(id: &str, selector: &str, subject: &str) -> Vec<u8> {
    format!("{id}\x1f{selector}\x1f{subject}\0").into_bytes()
}

#[test]
fn the_list_reads_each_stash_with_its_selector_and_skips_what_does_not_parse() {
    let mut bytes = record(A, "stash@{0}", "On main: tidy");
    bytes.extend(record("zz", "stash@{1}", "bad id"));
    bytes.extend(record(B, "refs/stash", "bad selector"));
    bytes.extend(record(B, "stash@{2}", "WIP on main: 1234567 base"));
    let log = parse(&bytes, 10);
    let rows: Vec<(&str, &[u8])> = (log.commits.iter())
        .map(|commit| (commit.short.as_str(), commit.subject.as_slice()))
        .collect();
    assert_eq!(
        rows,
        [
            ("stash@{0}", &b"On main: tidy"[..]),
            ("stash@{2}", &b"WIP on main: 1234567 base"[..]),
        ]
    );
    assert!(!log.more);
    let cut = parse(&bytes, 1);
    assert_eq!((cut.commits.len(), cut.more), (1, true));
}

#[test]
fn a_selector_is_stash_and_a_number() {
    assert!(is_selector("stash@{0}"));
    assert!(is_selector("stash@{12}"));
    for not in [
        "stash@{}",
        "stash@{-1}",
        "stash@{1",
        "refs/stash",
        "stash@{1}x",
    ] {
        assert!(!is_selector(not), "{not}");
    }
}

#[test]
fn a_stash_is_found_again_where_it_is_now() {
    let listed = format!("{B}\0{A}\0");
    assert_eq!(find(listed.as_bytes(), A).as_deref(), Some("stash@{1}"));
    assert_eq!(find(listed.as_bytes(), B).as_deref(), Some("stash@{0}"));
    assert_eq!(find(b"", A), None);
}

#[test]
fn a_push_takes_its_message_as_one_line_and_untracked_files_if_asked() {
    assert_eq!(push_args(false, "  "), ["stash", "push"]);
    assert_eq!(
        push_args(true, "fix\nit\u{1b}[2J"),
        [
            "stash",
            "push",
            "--include-untracked",
            "--message=fix it [2J"
        ]
    );
    let long = "é".repeat(2000);
    let words = push_args(false, &long);
    assert!(
        words[2].len() <= "--message=".len() + super::MAX_MESSAGE,
        "{}",
        words[2].len()
    );
}

#[test]
fn each_act_names_the_stash_now_at_its_selector() {
    assert_eq!(args(Act::Pop, "stash@{1}"), ["stash", "pop", "stash@{1}"]);
    assert_eq!(title(Act::Drop, "stash@{0}"), "git stash drop stash@{0}");
    assert_eq!(
        Act::Apply.done("stash@{0}"),
        "Applied stash@{0}; it is kept"
    );
}

#[test]
fn a_stash_that_does_not_apply_cleanly_says_it_is_kept() {
    let said = failed(
        Act::Pop,
        "git stash pop stash@{0}",
        "CONFLICT (content): a.txt\n",
    );
    assert!(said.contains("the stash is kept"), "{said}");
    let said = failed(
        Act::Drop,
        "git stash drop stash@{0}",
        "error: no such stash\n",
    );
    assert!(!said.contains("kept"), "{said}");
}

#[test]
fn the_drop_question_escapes_the_subject() {
    let question = drop_question("stash@{0}", "On main: a\u{202e}b\nc".as_bytes());
    assert!(
        question.starts_with("Drop stash@{0} On main: "),
        "{question}"
    );
    assert!(!question.contains(['\u{202e}', '\n']), "{question:?}");
}

proptest! {
    #[test]
    fn any_bytes_never_panic_and_rows_stay_whole(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let log = parse(&bytes, 4);
        prop_assert!(log.commits.len() <= 4);
        for commit in &log.commits {
            prop_assert!(is_selector(&commit.short));
            prop_assert!(matches!(commit.id.len(), 40 | 64));
        }
    }
}
