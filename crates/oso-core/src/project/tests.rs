//! The tests of the project rules: markers, `.git` files and worktree links.
//!
//! Must not depend on: `buffer`, `editor` or file I/O.

use super::*;

fn dirs<'a>(list: &'a [(&'a str, Markers)]) -> impl Iterator<Item = (&'a Path, Markers)> {
    list.iter()
        .map(|(dir, markers)| (Path::new(*dir), *markers))
}

const GIT: Markers = Markers {
    git: Some(GitMarker::Dir),
    jj: false,
    hg: false,
};
const NONE: Markers = Markers {
    git: None,
    jj: false,
    hg: false,
};

#[test]
fn the_nearest_marker_wins() {
    let list = [
        ("/w/oso/vendor/lib/src", NONE),
        ("/w/oso/vendor/lib", GIT),
        ("/w/oso", GIT),
    ];
    assert_eq!(
        nearest_root(dirs(&list)),
        Some((Path::new("/w/oso/vendor/lib"), GIT))
    );
}

#[test]
fn each_marker_makes_a_root_and_none_makes_no_project() {
    let jj = Markers { jj: true, ..NONE };
    let hg = Markers { hg: true, ..NONE };
    let file = Markers {
        git: Some(GitMarker::File),
        ..NONE
    };
    for markers in [jj, hg, file, GIT] {
        assert!(markers.any());
        assert_eq!(
            nearest_root(dirs(&[("/a", markers)])),
            Some((Path::new("/a"), markers))
        );
    }
    assert_eq!(nearest_root(dirs(&[("/a/b", NONE), ("/a", NONE)])), None);
}

#[test]
fn the_search_stops_at_the_first_root() {
    let mut seen = 0;
    let found = nearest_root(
        [("/a/b", GIT), ("/a", GIT)]
            .into_iter()
            .map(|(dir, markers)| {
                seen += 1;
                (Path::new(dir), markers)
            }),
    );
    assert_eq!(found, Some((Path::new("/a/b"), GIT)));
    assert_eq!(seen, 1);
}

#[test]
fn a_git_file_names_its_repository_on_its_first_line() {
    assert_eq!(
        git_file_target(b"gitdir: /w/oso/.git/worktrees/x\n"),
        Some(PathBuf::from("/w/oso/.git/worktrees/x"))
    );
    assert_eq!(
        git_file_target(b"gitdir: ../.git/modules/lib\r\nmore"),
        Some(PathBuf::from("../.git/modules/lib"))
    );
    for bad in [
        &b"gitdir:/x"[..],
        b"gitdir: ",
        b"something else",
        b"",
        b"gitdir: /x\0",
        b"gitdir: \xff",
    ] {
        assert_eq!(git_file_target(bad), None, "{bad:?}");
    }
    let long = [b"gitdir: /".as_slice(), &[b'a'; 1100]].concat();
    assert_eq!(git_file_target(&long), None);
}

#[test]
fn a_backlink_is_the_first_line() {
    assert_eq!(
        worktree_backlink(b"/w/oso/.claude/worktrees/x/.git\n"),
        Some(PathBuf::from("/w/oso/.claude/worktrees/x/.git"))
    );
    assert_eq!(worktree_backlink(b"\n"), None);
}

#[test]
fn only_a_worktrees_directory_leads_to_a_main_repository() {
    assert_eq!(
        worktree_main(Path::new("/w/oso/.git/worktrees/x")),
        Some(Path::new("/w/oso"))
    );
    for other in [
        "/w/oso/.git/modules/lib",
        "/w/oso.git/worktrees/x",
        "/w/oso/.git/worktrees",
        "/w/oso/git/worktrees/x",
        "worktrees/x",
        "/",
    ] {
        assert_eq!(worktree_main(Path::new(other)), None, "{other}");
    }
}

#[test]
fn a_project_is_named_after_its_root() {
    let project = Project::new(Found {
        root: RootIdentity {
            path: PathBuf::from("/w/oso"),
            identity: Some((1, 2)),
        },
        trust_root: None,
    });
    assert_eq!(project.name(), "oso");
    assert_eq!(project.root(), Path::new("/w/oso"));
    let root = Project::new(Found {
        root: RootIdentity {
            path: PathBuf::from("/"),
            identity: None,
        },
        trust_root: None,
    });
    assert_eq!(root.name(), "/");
}
