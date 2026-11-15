//! The place of each kind of row of `*Sessions*`, for `c`, `w` and `RET`.
//!
//! Must not depend on: the app, a window or git.

use std::path::{Path, PathBuf};

use super::{Base, RowPlace, place_of};
use crate::sessions::list::{Group, Key, Line, Project, Tree, Worktree};

fn tree() -> Tree {
    let worktree = |path: &str, branch: Option<&str>, primary: bool| Worktree {
        path: PathBuf::from(path),
        branch: branch.map(str::to_owned),
        primary,
        ..Worktree::default()
    };
    Tree {
        spaces: Tree::of_projects(vec![Project {
            root: PathBuf::from("/p/oso"),
            name: "oso".to_owned(),
            worktrees: vec![
                worktree("/p/oso", Some("main"), true),
                worktree("/p/oso/.claude/worktrees/api", Some("api-fix"), false),
                worktree("/p/oso/.claude/worktrees/bare", None, false),
            ],
        }])
        .spaces,
        ..Tree::default()
    }
}

/// Session 7 runs in the worktree `api`, below its root; external sessions in `/q/x`.
fn place(line: &Line) -> Option<RowPlace> {
    place_of(
        line,
        &tree(),
        |key| (key == 7).then(|| PathBuf::from("/p/oso/.claude/worktrees/api/crates")),
        |_| Some(PathBuf::from("/q/x")),
    )
}

fn at(directory: &str, base: Base, session: Option<u64>) -> Option<RowPlace> {
    Some(RowPlace {
        directory: PathBuf::from(directory),
        repo: Some(PathBuf::from("/p/oso")),
        base,
        session,
        space: None,
    })
}

/// As [`at`], on a row of the tree's one space.
fn in_space(directory: &str, base: Base) -> Option<RowPlace> {
    at(directory, base, None).map(|place| RowPlace {
        space: Some(space()),
        ..place
    })
}

fn space() -> crate::sessions::spaces::SpaceId {
    tree().spaces[0].id.clone()
}

#[test]
fn a_project_is_its_main_checkout_from_head() {
    assert_eq!(place(&Line::Space(space())), in_space("/p/oso", Base::Head));
}

#[test]
fn a_worktree_is_itself_from_its_branch_or_head_when_detached() {
    let api = "/p/oso/.claude/worktrees/api";
    let place_api = place(&Line::Worktree(space(), PathBuf::from(api)));
    assert_eq!(place_api, in_space(api, Base::Branch("api-fix".into())));
    assert_eq!(place_api.as_ref().unwrap().base(), Some("api-fix"));
    let bare = "/p/oso/.claude/worktrees/bare";
    let detached = place(&Line::Worktree(space(), PathBuf::from(bare))).unwrap();
    assert!(detached.detached());
    assert_eq!(detached.base(), None);
    // The main checkout is not taken for a worktree inside it.
    assert_eq!(
        place(&Line::Worktree(space(), PathBuf::from("/p/oso"))),
        in_space("/p/oso", Base::Branch("main".into()))
    );
}

#[test]
fn a_session_is_its_worktree_and_its_parent() {
    assert_eq!(
        place(&Line::Session(Key::Hosted(7))),
        at(
            "/p/oso/.claude/worktrees/api",
            Base::Branch("api-fix".into()),
            Some(7)
        )
    );
    assert_eq!(place(&Line::Session(Key::Hosted(8))), None, "gone");
    // Outside the tree, an external session's own directory, in no project.
    let outside = place(&Line::Session(Key::External(42))).unwrap();
    assert_eq!(outside.directory, Path::new("/q/x"));
    assert!(outside.repo().is_err());
}

#[test]
fn only_an_other_projects_directory_of_the_groups_has_a_place() {
    let dir = place(&Line::Header(Group::OtherDir(PathBuf::from("/q/x")))).unwrap();
    assert_eq!(dir.directory, Path::new("/q/x"));
    for group in [Group::Other, Group::Archived, Group::State(0)] {
        assert_eq!(place(&Line::Header(group.clone())), None, "{group:?}");
    }
}
