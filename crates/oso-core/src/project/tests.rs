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
        checked_main: None,
    };
    let project = Project::new(found("/w/oso"));
    assert_eq!(project.name(), "oso");
    assert_eq!(project.root(), Path::new("/w/oso"));
    assert_eq!(Project::new(found("/")).name(), "/");
}

#[cfg(unix)]
#[test]
fn a_worktree_names_its_entry_in_the_main_repository() {
    assert_eq!(
        worktree_entry(Path::new("/w/wt"), b"gitdir: ../oso/.git/worktrees/wt\n"),
        Some(PathBuf::from("/w/oso/.git/worktrees/wt"))
    );
    assert_eq!(
        worktree_entry(Path::new("/w/sub"), b"gitdir: /w/oso/.git/modules/sub\n"),
        None
    );
}

#[cfg(unix)]
#[test]
fn a_backlink_is_its_first_line_by_its_words() {
    let entry = Path::new("/w/oso/.git/worktrees/wt");
    let back = |bytes: &[u8]| worktree_backlink(entry, bytes);
    assert_eq!(back(b"/w/wt/.git\n"), Some(PathBuf::from("/w/wt/.git")));
    assert_eq!(back(b"/w/wt/.git"), Some(PathBuf::from("/w/wt/.git")));
    assert_eq!(
        back(b"../../../../wt/./.git\r\nmore"),
        Some(PathBuf::from("/w/wt/.git")),
        "relative to the entry, as git's relative worktree paths are"
    );
    for bad in [&b""[..], b"\n/w/wt/.git", b"/w/wt/.git\0", b"/w/\xff/.git"] {
        assert_eq!(back(bad), None, "{bad:?}");
    }
    let long = [b"/w/wt/.git\n".as_slice(), &[b'#'; 1100]].concat();
    assert_eq!(back(&long), None);
}

#[test]
fn the_same_place_is_the_same_words() {
    let same = |a: &str, b: &str| same_place(Path::new(a), Path::new(b));
    assert!(same("/w/oso", "/w/oso"));
    assert!(same("/w/oso/", "/w//oso"), "as components");
    assert!(!same("/w/oso", "/w/oso/sub"));
    assert!(!same("/w/oso", "/w/Oso"));
}

/// Git for Windows writes plain paths with `/`; the roots oso records are verbatim.
#[cfg(windows)]
#[test]
fn a_verbatim_root_is_the_plain_path_git_writes() {
    let same = |a: &str, b: &str| same_place(Path::new(a), Path::new(b));
    assert!(same(r"\\?\C:\w\oso", "C:/w/oso"));
    assert!(same(r"\\?\C:\w\oso", r"c:\w\oso"));
    assert!(same(r"\\?\UNC\host\share\oso", r"\\host\share\oso"));
    assert!(!same(r"\\?\C:\w\oso", "D:/w/oso"));
    assert!(!same(r"\\?\C:\w\oso", r"\\host\C$\w\oso"));
}
