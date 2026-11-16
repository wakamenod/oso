//! Tests of a graph's records: what `git log` gives read into commits, the refs it names as
//! labels, the date at an offset, a page's arguments, and a property test over any bytes.
//!
//! Must not depend on: git or the file system.

use proptest::prelude::*;

use super::{Kind, Label, PAGE, args, date, labels, parse};

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn record(id: &str, parents: &str, refs: &str, subject: &str) -> Vec<u8> {
    format!(
        "{id}\x1f{}\x1f{parents}\x1f{refs}\x1fAuthor\x1f86400\x1f{subject}\0",
        &id[..id.len().min(7)]
    )
    .into_bytes()
}

#[test]
fn a_page_reads_each_commit_and_says_there_are_more() {
    let mut bytes = record(A, &format!("{B} {C}"), "HEAD -> refs/heads/main", "Merge");
    bytes.extend(record(B, "", "", "Root"));
    bytes.extend(record("zz", "", "", "not hex"));
    bytes.extend(record(C, "", "", "Another"));
    let (commits, more) = parse(&bytes, 2);
    assert!(more);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].parents, [B, C]);
    assert_eq!(commits[0].short, &A[..7]);
    assert_eq!(commits[0].subject, b"Merge");
    assert_eq!(commits[1].parents, Vec::<String>::new());
    let (all, more) = parse(&bytes, 10);
    assert_eq!((all.len(), more), (3, false));
}

#[test]
fn a_parent_that_is_not_hex_skips_its_record() {
    let bytes = record(A, "--all", "", "x");
    assert_eq!(parse(&bytes, 10).0, Vec::new());
}

#[test]
fn the_labels_name_head_branches_remotes_and_tags() {
    let got = labels(
        b"HEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, \
          tag: refs/tags/v1, refs/stash",
    );
    let want = [
        (Kind::Head, "HEAD"),
        (Kind::Local, "main"),
        (Kind::Remote, "origin/main"),
        (Kind::Tag, "v1"),
    ];
    let want: Vec<Label> = (want.iter())
        .map(|(kind, name)| Label {
            kind: *kind,
            name: (*name).into(),
        })
        .collect();
    assert_eq!(got, want);
    assert_eq!(labels(b"HEAD")[0].kind, Kind::Head);
}

#[test]
fn a_date_is_the_local_day() {
    assert_eq!(date(0, 0), "1970-01-01");
    assert_eq!(date(86_399, 3600), "1970-01-02");
    assert_eq!(date(0, -3600), "1969-12-31");
}

#[test]
fn a_page_s_arguments_name_the_refs_after_its_options() {
    let words = args(&["refs/heads/main".into()], 4000);
    assert_eq!(words[0], "log");
    assert!(words.contains(&"--topo-order".to_owned()));
    assert!(words.contains(&"--skip=4000".to_owned()));
    assert!(words.contains(&format!("--max-count={}", PAGE + 1)));
    assert_eq!(&words[words.len() - 2..], ["refs/heads/main", "--"]);
}

proptest! {
    #[test]
    fn any_bytes_never_panic_and_ids_stay_hex(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
        let (commits, _) = parse(&bytes, 8);
        prop_assert!(commits.len() <= 8);
        for commit in commits {
            prop_assert!(commit.id.bytes().all(|b| b.is_ascii_hexdigit()));
            for parent in commit.parents {
                prop_assert!(parent.bytes().all(|b| b.is_ascii_hexdigit()));
            }
        }
    }
}
