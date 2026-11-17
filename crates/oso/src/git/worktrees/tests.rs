//! Tests of the worktree list's parser: git's `-z` output as literal bytes.
//!
//! Must not depend on: git, processes or the file system.

use std::path::PathBuf;

use proptest::prelude::*;

use super::{ListedWorktree, worktrees};

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
            ListedWorktree {
                path: PathBuf::from("/repo"),
                head: Some("1111".into()),
                branch: Some("main".into()),
                ..ListedWorktree::default()
            },
            ListedWorktree {
                path: PathBuf::from("/repo/.claude/worktrees/x"),
                head: Some("2222".into()),
                detached: true,
                locked: true,
                ..ListedWorktree::default()
            },
            ListedWorktree {
                path: PathBuf::from("/gone"),
                head: Some("3333".into()),
                branch: Some("feat/a".into()),
                prunable: true,
                ..ListedWorktree::default()
            },
            ListedWorktree {
                path: PathBuf::from("/bare"),
                bare: true,
                ..ListedWorktree::default()
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

proptest! {
    #[test]
    fn no_listing_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512), max in 0usize..8) {
        prop_assert!(worktrees(&bytes, max).len() <= max);
    }
}
