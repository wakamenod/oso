//! Tests of `porcelain`: git's `-z` outputs as literal bytes, and arbitrary bytes, which no
//! parser panics on.
//!
//! Must not depend on: git, processes or the file system.

use std::path::PathBuf;

use proptest::prelude::*;

use super::{Status, made_a_commit, names, paths, ref_pairs, status, status_lines};

#[test]
fn status_counts() {
    let bytes = b" M src/a.rs\0?? new.txt\0?? dir/b\0!! target/\0R  new.rs\0old.rs\0C  copy\0orig\0UU both\0A  added\0";
    assert_eq!(
        status(bytes),
        Status {
            changed: 5,
            untracked: 2
        }
    );
    assert_eq!(status(b""), Status::default());
    // The old path of a rename is skipped, even when it looks like an entry.
    assert_eq!(
        status(b"RM x\0?? y\0"),
        Status {
            changed: 1,
            untracked: 0
        }
    );
    // Short junk is skipped; a cut final entry still counts.
    assert_eq!(
        status(b"M\0 M a\0 D b"),
        Status {
            changed: 2,
            untracked: 0
        }
    );
}

#[test]
fn status_lines_are_printable() {
    let bytes = b" M a\x1b[31m.rs\0R  new\0old\0?? z\0";
    assert_eq!(
        status_lines(bytes, 10),
        [" M a\\u{1b}[31m.rs", "R  old -> new", "?? z"]
    );
    assert_eq!(status_lines(bytes, 1).len(), 1);
}

#[test]
fn ls_files_paths() {
    assert_eq!(
        paths(b"a.txt\0dir/\0\0b c\0", 10),
        [
            PathBuf::from("a.txt"),
            PathBuf::from("dir/"),
            PathBuf::from("b c")
        ]
    );
    assert_eq!(paths(b"a\0b\0", 1), [PathBuf::from("a")]);
}

#[test]
fn ref_names() {
    assert_eq!(
        names(b"main\0\nfeat/x\0\n\xff\0\n\0\n", 10),
        ["main", "feat/x"]
    );
    assert_eq!(names(b"a\0\nb\0\n", 1), ["a"]);
    assert!(names(b"", 10).is_empty());
}

#[test]
fn remote_ref_pairs() {
    let bytes = b"refs/remotes/origin/HEAD\0refs/remotes/origin/main\0\n\
refs/remotes/origin/main\0\0\n\
\0\0\n\
refs/remotes/up/dev\0\0\n";
    assert_eq!(
        ref_pairs(bytes, 10),
        [
            (
                "refs/remotes/origin/HEAD".to_owned(),
                Some("refs/remotes/origin/main".to_owned())
            ),
            ("refs/remotes/origin/main".to_owned(), None),
            ("refs/remotes/up/dev".to_owned(), None),
        ]
    );
    assert_eq!(ref_pairs(bytes, 1).len(), 1);
}

#[test]
fn reflogs_with_and_without_commits() {
    assert!(made_a_commit(b"commit: fix\0branch: Created from HEAD\0"));
    for subject in [
        "commit (amend): x",
        "commit (merge): y",
        "commit (initial): z",
        "cherry-pick: w",
        "revert: v",
    ] {
        let reflog = format!("branch: Created from main\0\n{subject}\0");
        assert!(made_a_commit(reflog.as_bytes()), "{subject}");
    }
    assert!(!made_a_commit(
        b"branch: Created from HEAD\0reset: moving to HEAD~1\0rebase (finish): x\0"
    ));
    assert!(!made_a_commit(b""));
}

proptest! {
    #[test]
    fn no_parser_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512), max in 0usize..8) {
        let counts = status(&bytes);
        prop_assert!(counts.changed + counts.untracked <= bytes.len());
        prop_assert!(status_lines(&bytes, max).len() <= max);
        prop_assert!(paths(&bytes, max).len() <= max);
        prop_assert!(names(&bytes, max).len() <= max);
        prop_assert!(ref_pairs(&bytes, max).len() <= max);
        let _ = made_a_commit(&bytes);
    }
}
