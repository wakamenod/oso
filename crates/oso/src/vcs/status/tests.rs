//! What `git status --porcelain=v2` and `git diff --numstat` say, read: each kind of record,
//! odd paths, records that do not parse, and any bytes.
//!
//! Must not depend on: `app`, `dispatch`, git or winit; these are pure functions' tests.

use proptest::prelude::*;

use super::{Branch, Kind, parse, parse_numstat};

const OID: &str = "1111111111111111111111111111111111111111";

fn record(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(part);
        out.push(0);
    }
    out
}

#[test]
fn the_branch_headers_are_read() {
    let bytes = record(&[
        format!("# branch.oid {OID}").as_bytes(),
        b"# branch.head main",
        b"# branch.upstream origin/main",
        b"# branch.ab +2 -13",
    ]);
    assert_eq!(
        parse(&bytes).branch,
        Branch {
            oid: Some(OID.into()),
            head: Some(b"main".to_vec()),
            upstream: Some(b"origin/main".to_vec()),
            ahead_behind: Some((2, 13)),
        }
    );
}

#[test]
fn an_initial_or_detached_head_is_none() {
    let bytes = record(&[b"# branch.oid (initial)", b"# branch.head (detached)"]);
    let branch = parse(&bytes).branch;
    assert_eq!(branch.oid, None);
    assert_eq!(branch.head, None);
    assert_eq!(branch.ahead_behind, None);
}

#[test]
fn each_kind_of_record_is_read_with_spaces_in_its_path() {
    let bytes = record(&[
        b"1 .M N... 100644 100644 100644 aaaa bbbb src/a file.rs",
        b"1 A. N... 000000 100644 100644 0000 cccc new.rs",
        b"2 R. N... 100644 100644 100644 aaaa aaaa R100 new name.rs",
        b"old name.rs",
        b"u UU N... 100644 100644 100644 100644 a b c both.rs",
        b"? notes/todo.txt",
        b"1 .M S.M. 160000 160000 160000 aaaa aaaa vendor/lib",
    ]);
    let entries = parse(&bytes).entries;
    assert_eq!(entries.len(), 6);
    assert_eq!(entries[0].path, b"src/a file.rs");
    assert_eq!((entries[0].staged, entries[0].unstaged), (b'.', b'M'));
    assert_eq!(entries[1].staged, b'A');
    assert_eq!(
        entries[2].kind,
        Kind::Moved {
            from: b"old name.rs".to_vec(),
            copied: false
        }
    );
    assert_eq!(entries[2].path, b"new name.rs");
    assert_eq!(entries[3].kind, Kind::Unmerged);
    assert_eq!(entries[3].path, b"both.rs");
    assert_eq!(entries[4].kind, Kind::Untracked);
    assert_eq!(entries[4].path, b"notes/todo.txt");
    assert!(entries[5].submodule);
    assert!(!entries[0].submodule);
}

#[test]
fn a_path_that_is_not_utf8_or_holds_a_line_break_stays_bytes() {
    let mut odd = b"1 .M N... 100644 100644 100644 aaaa bbbb a\nb\xff".to_vec();
    odd.push(0);
    let entries = parse(&odd).entries;
    assert_eq!(entries[0].path, b"a\nb\xff");
}

#[test]
fn a_record_that_does_not_parse_is_skipped() {
    let bytes = record(&[
        b"1 .M N... 100644",
        b"1 .MX N... 100644 100644 100644 aaaa bbbb long-letters",
        b"1 .M N... 100644 100644 100644 aaaa bbbb ",
        b"2 R. N... 100644 100644 100644 aaaa aaaa R100 lost-its-origin",
        b"? ",
        b"1 .M N... 100644 100644 100644 aaaa bbbb kept.rs",
    ]);
    let entries = parse(&bytes).entries;
    // The rename takes the next record as its origin, whatever it holds.
    let paths: Vec<&[u8]> = entries.iter().map(|entry| entry.path.as_slice()).collect();
    assert_eq!(paths, [b"lost-its-origin".as_slice(), b"kept.rs"]);
}

#[test]
fn numstat_counts_each_path_and_a_rename_by_its_new_path() {
    let bytes = record(&[
        b"3\t1\tsrc/a.rs",
        b"-\t-\timage.png",
        b"0\t0\t",
        b"old.rs",
        b"new.rs",
        b"x\t1\tbad.rs",
    ]);
    let counts = parse_numstat(&bytes);
    assert_eq!(counts.get(b"src/a.rs".as_slice()), Some(&Some((3, 1))));
    assert_eq!(counts.get(b"image.png".as_slice()), Some(&None));
    assert_eq!(counts.get(b"new.rs".as_slice()), Some(&Some((0, 0))));
    assert!(!counts.contains_key(b"old.rs".as_slice()));
    assert!(!counts.contains_key(b"bad.rs".as_slice()));
}

proptest! {
    // The parsers' input is a child's output: any bytes give entries no larger than it, and
    // never a panic.
    #[test]
    fn any_bytes_parse_within_their_size(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let status = parse(&bytes);
        let paths: usize = status.entries.iter().map(|entry| entry.path.len()).sum();
        prop_assert!(paths <= bytes.len());
        prop_assert!(status.entries.len() <= bytes.len());
        let counts = parse_numstat(&bytes);
        prop_assert!(counts.len() <= bytes.len());
    }

    #[test]
    fn records_made_of_the_alphabet_parse_too(
        parts in proptest::collection::vec(
            proptest::sample::select(vec![
                b"1".to_vec(), b"2".to_vec(), b"u".to_vec(), b"?".to_vec(), b"#".to_vec(),
                b" ".to_vec(), b"\0".to_vec(), b".M".to_vec(), b"N...".to_vec(), b"R100".to_vec(),
                b"\t".to_vec(), b"-".to_vec(), b"+1".to_vec(), b"branch.ab".to_vec(), b"a".to_vec(),
            ]),
            0..64,
        )
    ) {
        let bytes = parts.concat();
        let status = parse(&bytes);
        prop_assert!(status.entries.iter().all(|entry| !entry.path.is_empty()));
        let _ = parse_numstat(&bytes);
    }
}
