//! Tests of the commit sections' reader: records read in order, a cut at the count kept, bad
//! records skipped, and any bytes taken.
//!
//! Must not depend on: `app`, `dispatch` or winit; nothing here runs git.

use proptest::prelude::*;

use super::{args, parse};

const A: &str = "0123456789abcdef0123456789abcdef01234567";
const B: &str = "89abcdef0123456789abcdef0123456789abcdef";

fn record(id: &str, short: &str, subject: &[u8]) -> Vec<u8> {
    let mut bytes = format!("{id}\x1f{short}\x1f").into_bytes();
    bytes.extend_from_slice(subject);
    bytes.push(0);
    bytes
}

#[test]
fn records_are_read_in_order_with_their_subjects_as_bytes() {
    let mut out = record(A, "0123456", b"first");
    out.extend(record(B, "89abcde", b"a \x1f in it\n\xff"));
    let log = parse(&out, 10);
    assert!(!log.more);
    assert_eq!(log.commits.len(), 2);
    assert_eq!(log.commits[0].id, A);
    assert_eq!(log.commits[0].short, "0123456");
    assert_eq!(log.commits[0].subject, b"first");
    // A unit separator in a subject stays in it.
    assert_eq!(log.commits[1].subject, b"a \x1f in it\n\xff");
}

#[test]
fn past_the_count_kept_it_says_there_are_more() {
    let mut out = record(A, "0123456", b"first");
    out.extend(record(B, "89abcde", b"second"));
    let log = parse(&out, 1);
    assert!(log.more);
    assert_eq!(log.commits.len(), 1);
}

#[test]
fn a_record_whose_ids_are_not_hex_is_skipped() {
    let mut out = record("-n", "0123456", b"bad");
    out.extend(record(A, "zz", b"bad too"));
    out.extend(record(A, "0123456", b"good"));
    out.extend(b"no separators\0".iter());
    let log = parse(&out, 10);
    assert_eq!(log.commits.len(), 1);
    assert_eq!(log.commits[0].subject, b"good");
}

#[test]
fn a_long_subject_is_cut_at_a_character() {
    let long = "é".repeat(super::MAX_SUBJECT);
    let log = parse(&record(A, "0123456", long.as_bytes()), 1);
    let subject = String::from_utf8(log.commits[0].subject.clone()).unwrap();
    assert!(subject.ends_with('…'));
    assert!(subject.len() <= super::MAX_SUBJECT + "…".len());
}

#[test]
fn the_arguments_read_one_more_than_kept() {
    let words = args("@{upstream}..HEAD", 256);
    assert!(words.contains(&"--max-count=257".to_owned()));
    assert_eq!(words.last().map(String::as_str), Some("--"));
    assert!(words.contains(&"--no-show-signature".to_owned()));
}

proptest! {
    #[test]
    fn any_bytes_are_read_without_panicking(bytes in proptest::collection::vec(any::<u8>(), 0..512), keep in 0usize..5) {
        let log = parse(&bytes, keep);
        prop_assert!(log.commits.len() <= keep);
        for commit in &log.commits {
            prop_assert!(commit.id.bytes().all(|b| b.is_ascii_hexdigit()));
            prop_assert!(commit.short.len() <= commit.id.len());
        }
    }
}
