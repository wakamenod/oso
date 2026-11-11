//! The tests of the project rules: what a worktree's `.git` file names, by its words.
//!
//! Must not depend on: `buffer`, `editor` or file I/O.

use super::*;

/// An absolute path on this system: `path` as written on Unix, and on drive `C:` on Windows,
/// where a path with no drive is not absolute.
fn abs(path: &str) -> String {
    if cfg!(windows) {
        format!("C:{path}")
    } else {
        path.to_owned()
    }
}

/// What a `.git` file of `line` names, at the root `root` (made absolute by [`abs`]).
fn claim(root: &str, line: &str) -> Option<PathBuf> {
    worktree_claim(Path::new(&abs(root)), line.as_bytes())
}

fn path(written: &str) -> Option<PathBuf> {
    Some(PathBuf::from(abs(written)))
}

#[test]
fn a_worktree_names_its_main_repository() {
    let line = format!("gitdir: {}\n", abs("/w/oso/.git/worktrees/x"));
    assert_eq!(claim("/w/oso/.claude/worktrees/x", &line), path("/w/oso"));
    assert_eq!(
        claim("/w/wt", "gitdir: ../oso/.git/worktrees/wt\r\nmore"),
        path("/w/oso")
    );
    let line = format!("gitdir: {}", abs("/w/./oso/x/../.git/worktrees/wt"));
    assert_eq!(claim("/w/wt", &line), path("/w/oso"));
}

#[cfg(windows)]
#[test]
fn a_windows_gitdir_line_names_its_main_repository() {
    // Git for Windows writes the drive and forward slashes; backslashes name the same place.
    assert_eq!(
        claim("/w/wt", "gitdir: C:/w/oso/.git/worktrees/wt\n"),
        path("/w/oso")
    );
    assert_eq!(
        claim("/w/wt", "gitdir: C:\\w\\oso\\.git\\worktrees\\wt\n"),
        path("/w/oso")
    );
}

#[test]
fn anything_but_a_worktrees_directory_names_none() {
    for named in [
        abs("/w/oso/.git/modules/lib"),
        abs("/w/oso.git/worktrees/x"),
        abs("/w/oso/.git/worktrees"),
        abs("/w/oso/git/worktrees/x"),
        abs("/"),
        "../../../../../../.git/worktrees/x".to_owned(),
    ] {
        let line = format!("gitdir: {named}");
        assert_eq!(claim("/w/a", &line), None, "{line}");
    }
}

#[test]
fn only_a_short_first_line_of_gitdir_counts() {
    let root = PathBuf::from(abs("/w/a"));
    let named = abs("/x/.git/worktrees/a");
    for bad in [
        format!("gitdir:{named}").into_bytes(),
        b"gitdir: ".to_vec(),
        b"something else".to_vec(),
        Vec::new(),
        format!("gitdir: {named}\0").into_bytes(),
        [b"gitdir: ".as_slice(), b"\xff", named.as_bytes()].concat(),
    ] {
        assert_eq!(worktree_claim(&root, &bad), None, "{bad:?}");
    }
    let long = [format!("gitdir: {named}").as_bytes(), &[b'#'; 1100]].concat();
    assert_eq!(worktree_claim(&root, &long), None);
}

#[test]
fn a_relative_root_names_none() {
    let claimed = worktree_claim(Path::new("w/a"), b"gitdir: ../oso/.git/worktrees/a");
    assert_eq!(claimed, None);
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
    let project = Project::new(found(&abs("/w/oso")));
    assert_eq!(project.name(), "oso");
    assert_eq!(project.root(), Path::new(&abs("/w/oso")));
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
