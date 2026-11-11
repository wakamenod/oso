//! The tests of the project rules: what a worktree's `.git` file names, by its words.
//!
//! Must not depend on: `buffer`, `editor` or file I/O.

use super::*;

fn claim(root: &str, bytes: &[u8]) -> Option<PathBuf> {
    worktree_claim(Path::new(root), bytes)
}

#[test]
fn a_worktree_names_its_main_repository() {
    assert_eq!(
        claim(
            "/w/oso/.claude/worktrees/x",
            b"gitdir: /w/oso/.git/worktrees/x\n"
        ),
        Some(PathBuf::from("/w/oso"))
    );
    assert_eq!(
        claim("/w/wt", b"gitdir: ../oso/.git/worktrees/wt\r\nmore"),
        Some(PathBuf::from("/w/oso"))
    );
    assert_eq!(
        claim("/w/wt", b"gitdir: /w/./oso/x/../.git/worktrees/wt"),
        Some(PathBuf::from("/w/oso"))
    );
}

#[test]
fn anything_but_a_worktrees_directory_names_none() {
    for gitdir in [
        "gitdir: /w/oso/.git/modules/lib",
        "gitdir: /w/oso.git/worktrees/x",
        "gitdir: /w/oso/.git/worktrees",
        "gitdir: /w/oso/git/worktrees/x",
        "gitdir: /",
        "gitdir: ../../../../../../.git/worktrees/x",
    ] {
        assert_eq!(claim("/w/a", gitdir.as_bytes()), None, "{gitdir}");
    }
}

#[test]
fn only_a_short_first_line_of_gitdir_counts() {
    for bad in [
        &b"gitdir:/x/.git/worktrees/a"[..],
        b"gitdir: ",
        b"something else",
        b"",
        b"gitdir: /x/.git/worktrees/a\0",
        b"gitdir: /\xff/.git/worktrees/a",
    ] {
        assert_eq!(claim("/w/a", bad), None, "{bad:?}");
    }
    let long = [b"gitdir: /x/.git/worktrees/a".as_slice(), &[b'#'; 1100]].concat();
    assert_eq!(claim("/w/a", &long), None);
}

#[test]
fn a_relative_root_names_none() {
    assert_eq!(claim("w/a", b"gitdir: ../oso/.git/worktrees/a"), None);
}

#[test]
fn a_project_is_named_after_its_root() {
    let found = |path: &str| Found {
        root: RootIdentity {
            path: PathBuf::from(path),
            identity: None,
        },
        worktree_of: None,
    };
    let project = Project::new(found("/w/oso"));
    assert_eq!(project.name(), "oso");
    assert_eq!(project.root(), Path::new("/w/oso"));
    assert_eq!(Project::new(found("/")).name(), "/");
}
