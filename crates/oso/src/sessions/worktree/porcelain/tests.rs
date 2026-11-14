//! Tests of `porcelain`: git's `-z` outputs as literal bytes, and arbitrary bytes, which no
//! parser panics on.
//!
//! Must not depend on: git, processes or the file system.

use std::path::PathBuf;

use proptest::prelude::*;

use super::{
    Listed, Status, made_a_commit, names, paths, printable, ref_pairs, status, status_lines,
    worktrees,
};

#[test]
fn worktree_records_are_read() {
    let bytes = b"worktree /repo\0HEAD 1111\0branch refs/heads/main\0\0\
worktree /repo/.claude/worktrees/x\0HEAD 2222\0detached\0locked reason here\0\0\
worktree /gone\0HEAD 3333\0branch refs/heads/feat/a\0prunable gitdir file points to non-existent location\0\0\
worktree /bare\0bare\0\0";
    let listed = worktrees(bytes, 10);
    assert_eq!(
        listed,
        vec![
            Listed {
                path: PathBuf::from("/repo"),
                head: Some("1111".into()),
                branch: Some("main".into()),
                ..Listed::default()
            },
            Listed {
                path: PathBuf::from("/repo/.claude/worktrees/x"),
                head: Some("2222".into()),
                detached: true,
                locked: true,
                ..Listed::default()
            },
            Listed {
                path: PathBuf::from("/gone"),
                head: Some("3333".into()),
                branch: Some("feat/a".into()),
                prunable: true,
                ..Listed::default()
            },
            Listed {
                path: PathBuf::from("/bare"),
                bare: true,
                ..Listed::default()
            },
        ]
    );
    assert_eq!(worktrees(bytes, 2).len(), 2);
    assert!(worktrees(bytes, 0).is_empty());
}

#[test]
fn odd_worktree_records() {
    // No `worktree` field: dropped. An unknown field: ignored. A bare `locked`: locked.
    // `lockedx` is no flag. A path with a line break and spaces is kept as it is.
    let bytes =
        b"HEAD 1\0branch refs/heads/x\0\0worktree /a b\nc\0future thing\0locked\0lockedx\0\0";
    let listed = worktrees(bytes, 10);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, PathBuf::from("/a b\nc"));
    assert!(listed[0].locked);
    // A record git did not end, and a field cut short.
    let listed = worktrees(
        b"worktree /a\0HEAD 1\0worktree /b\0HEAD 2\0branch refs/he",
        10,
    );
    assert_eq!(
        listed.iter().map(|l| l.path.clone()).collect::<Vec<_>>(),
        [PathBuf::from("/a"), PathBuf::from("/b")]
    );
    assert_eq!(listed[1].branch, None);
    // Output cut inside the first `worktree` field names no worktree.
    assert!(worktrees(b"worktree /rep", 10).is_empty());
}

#[cfg(unix)]
#[test]
fn worktree_paths_keep_their_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let listed = worktrees(b"worktree /a\xff\0branch refs/heads/\xfe\0\0", 10);
    assert_eq!(listed[0].path.as_os_str().as_bytes(), b"/a\xff");
    assert_eq!(listed[0].branch, None);
}

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

#[test]
fn printable_escapes_and_cuts() {
    assert_eq!(printable("plain é 日本", 100), "plain é 日本");
    assert_eq!(printable("a\nb\tc\u{7f}", 100), "a\\u{a}b\\u{9}c\\u{7f}");
    assert_eq!(
        printable("\u{9b}\u{2028}\u{202e}\u{2067}", 100),
        "\\u{9b}\\u{2028}\\u{202e}\\u{2067}"
    );
    assert_eq!(printable("abcdef", 3), "abc…");
    assert_eq!(printable("abc", 3), "abc");
    // An escape is not split.
    assert_eq!(printable("ab\u{1b}", 4), "ab…");
    assert_eq!(printable("x", 0), "…");
}

proptest! {
    #[test]
    fn no_parser_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512), max in 0usize..8) {
        let listed = worktrees(&bytes, max);
        prop_assert!(listed.len() <= max);
        let counts = status(&bytes);
        prop_assert!(counts.changed + counts.untracked <= bytes.len());
        prop_assert!(status_lines(&bytes, max).len() <= max);
        prop_assert!(paths(&bytes, max).len() <= max);
        prop_assert!(names(&bytes, max).len() <= max);
        prop_assert!(ref_pairs(&bytes, max).len() <= max);
        let _ = made_a_commit(&bytes);
    }

    #[test]
    fn printable_shows_no_control(text in any::<String>(), max in 0usize..64) {
        let shown = printable(&text, max);
        prop_assert!(shown.chars().count() <= max + 1);
        prop_assert!(!shown.chars().any(super::hidden));
    }
}
