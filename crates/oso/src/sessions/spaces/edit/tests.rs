//! The space commands' changes: a new space named after its issue, its repositories, a name
//! taken from its directory, renaming, and archiving and reopening it, with the space entered
//! last taking the frame (ADR 0136, step 1b).
//!
//! Must not depend on: `app` or `dispatch`; these drive an editor alone, with no window.

use std::path::{Path, PathBuf};

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;

use super::NotAdded;
use crate::sessions::spaces::stored::MAX_MEMBERS;
use crate::sessions::spaces::{Member, SpaceId, Spaces, Stored};

fn id(n: u8) -> SpaceId {
    SpaceId::from_random([n; 16])
}

fn recalled(ids: u8) -> Spaces {
    let mut spaces = Spaces::default();
    spaces.recall(Vec::new(), (1..=ids).map(id).collect(), false);
    spaces
}

fn editor() -> Editor {
    Editor::with_buffer(Buffer::from_text("a.rs", "a\n"))
}

#[test]
fn a_new_space_is_named_after_its_issue_with_its_title_cleaned() {
    let mut spaces = recalled(3);
    let at = spaces
        .new_space(
            " Fix\u{200B} the  login\u{1b} error ",
            Member::at(Path::new("/b")),
            5,
        )
        .unwrap();
    let stored = spaces.stored(at).unwrap();
    assert_eq!(stored.title, "Fix the login error");
    assert_eq!(stored.name, "fix-the-login-error");
    assert_eq!(stored.members, [Member::at(Path::new("/b"))]);
    let again = spaces
        .new_space("Fix the login error", Member::at(Path::new("/c")), 6)
        .unwrap();
    assert_eq!(spaces.stored(again).unwrap().name, "fix-the-login-error-2");
    let bare = spaces
        .new_space("\u{200B}", Member::at(Path::new("/src/app")), 7)
        .unwrap();
    assert_eq!(
        spaces.stored(bare).unwrap().name,
        "app",
        "named after its repository"
    );
    assert_eq!(
        spaces.new_space("more", Member::at(Path::new("/d")), 8),
        None,
        "no id left"
    );
}

#[test]
fn repositories_are_added_once_up_to_the_most_and_taken_out() {
    let mut spaces = recalled(1);
    let at = spaces
        .new_space("x", Member::at(Path::new("/r0")), 1)
        .unwrap();
    assert_eq!(
        spaces.add_member(at, Member::at(Path::new("/r0")), 2),
        Err(NotAdded::Already)
    );
    for n in 1..MAX_MEMBERS {
        let root = PathBuf::from(format!("/r{n}"));
        assert_eq!(spaces.add_member(at, Member::at(&root), 2), Ok(()));
    }
    assert_eq!(
        spaces.add_member(at, Member::at(Path::new("/one-more")), 2),
        Err(NotAdded::Full)
    );
    assert!(spaces.remove_member(at, Path::new("/r3"), 3));
    assert!(!spaces.remove_member(at, Path::new("/r3"), 3));
    spaces.set_identity(at, Path::new("/r0"), Some((1, 2)));
    assert_eq!(spaces.stored(at).unwrap().members[0].identity, Some((1, 2)));
    spaces.set_identity(at, Path::new("/r0"), Some((9, 9)));
    assert_eq!(
        spaces.stored(at).unwrap().members[0].identity,
        Some((1, 2)),
        "a kept identity is never replaced"
    );
}

#[test]
fn a_directory_made_under_another_name_gives_the_space_that_name() {
    let mut spaces = recalled(2);
    let a = spaces
        .new_space("oso", Member::at(Path::new("/a")), 1)
        .unwrap();
    let b = spaces
        .new_space("notes", Member::at(Path::new("/b")), 1)
        .unwrap();
    spaces.set_directory(
        a,
        PathBuf::from("/s/oso-2"),
        Some((3, 4)),
        "oso-2",
        "oso",
        2,
    );
    let stored = spaces.stored(a).unwrap();
    assert_eq!(stored.name, "oso-2");
    assert_eq!(stored.directory.as_deref(), Some(Path::new("/s/oso-2")));
    assert_eq!(stored.directory_identity, Some((3, 4)));
    // A name another space has is not taken.
    spaces.set_directory(b, PathBuf::from("/s/oso-2x"), None, "oso-2", "notes", 3);
    assert_eq!(spaces.stored(b).unwrap().name, "notes");
}

#[test]
fn renaming_makes_the_name_from_the_title_and_ignores_an_empty_one() {
    let mut spaces = recalled(2);
    let a = spaces
        .new_space("one", Member::at(Path::new("/a")), 1)
        .unwrap();
    spaces
        .new_space("two", Member::at(Path::new("/b")), 1)
        .unwrap();
    spaces.rename(a, "Two", 2);
    assert_eq!(spaces.stored(a).unwrap().name, "two-2");
    spaces.rename(a, "\u{200B}", 3);
    assert_eq!(spaces.stored(a).unwrap().title, "Two");
}

#[test]
fn closing_the_space_in_front_enters_the_one_entered_last_and_reopening_brings_it_back() {
    let mut editor = editor();
    let mut spaces = recalled(3);
    let a = spaces
        .new_space("a", Member::at(Path::new("/a")), 1)
        .unwrap();
    let b = spaces
        .new_space("b", Member::at(Path::new("/b")), 1)
        .unwrap();
    let c = spaces
        .new_space("c", Member::at(Path::new("/c")), 1)
        .unwrap();
    spaces.adopt(a);
    spaces.enter(&mut editor, b).unwrap();
    spaces.enter(&mut editor, c).unwrap();
    spaces.archive(&mut editor, c, 2).unwrap();
    assert_eq!(spaces.current(), Some(b), "the one entered last");
    assert!(spaces.stored(c).unwrap().archived);
    let listed: Vec<String> = spaces
        .archived_candidates()
        .iter()
        .map(|c| c.text().to_owned())
        .collect();
    assert_eq!(listed, ["c"]);
    assert_eq!(spaces.archived_named("c"), Some(c));
    spaces.archive(&mut editor, b, 3).unwrap();
    spaces.archive(&mut editor, a, 4).unwrap();
    assert_eq!(
        spaces.current(),
        None,
        "none open: the frame keeps its tree"
    );
    spaces.reopen(c, 5);
    assert!(!spaces.stored(c).unwrap().archived);
    assert_eq!(spaces.archived_named("c"), None);
}

#[test]
fn a_reopened_space_whose_name_was_taken_meanwhile_is_told_apart() {
    let mut editor = editor();
    let mut spaces = recalled(0);
    let old = Stored::of_repository(id(1), "fix".into(), Member::at(Path::new("/a")), 1);
    spaces.recall(vec![old], vec![id(2)], false);
    spaces.archive(&mut editor, 0, 2).unwrap();
    let new = spaces
        .new_space("fix", Member::at(Path::new("/b")), 3)
        .unwrap();
    assert_eq!(
        spaces.stored(new).unwrap().name,
        "fix-2",
        "the archived one keeps its name"
    );
    spaces.reopen(0, 4);
    assert_eq!(spaces.stored(0).unwrap().name, "fix");
}

#[test]
fn the_space_in_fronts_repositories_are_listed_by_name() {
    let mut spaces = recalled(1);
    let at = spaces
        .new_space("x", Member::at(Path::new("/src/back")), 1)
        .unwrap();
    spaces
        .add_member(at, Member::at(Path::new("/src/front")), 1)
        .unwrap();
    assert!(
        spaces.member_candidates().is_empty(),
        "no space in front yet"
    );
    spaces.adopt(at);
    let names: Vec<String> = spaces
        .member_candidates()
        .iter()
        .map(|c| c.text().to_owned())
        .collect();
    assert_eq!(names, ["back", "front"]);
    assert_eq!(
        spaces.member_named("front"),
        Some(PathBuf::from("/src/front"))
    );
    assert_eq!(
        spaces.members(),
        [PathBuf::from("/src/back"), PathBuf::from("/src/front")]
    );
}

#[test]
fn a_space_renamed_while_its_directory_was_made_keeps_its_new_name() {
    let mut spaces = recalled(1);
    let at = spaces
        .new_space("oso", Member::at(Path::new("/a")), 1)
        .unwrap();
    spaces.rename(at, "Other", 2);
    spaces.set_directory(at, PathBuf::from("/s/oso-2"), None, "oso-2", "oso", 3);
    assert_eq!(spaces.stored(at).unwrap().name, "other");
}

#[test]
fn closing_stops_the_idle_sessions_unless_one_works_or_needs_input() {
    use super::sessions_to_stop;
    use crate::sessions::state::State;
    let idle = vec![
        (1, "a".to_owned(), State::Idle),
        (2, "b".to_owned(), State::Done),
    ];
    assert_eq!(sessions_to_stop(idle), Ok(vec![1, 2]));
    let busy = vec![
        (1, "a".to_owned(), State::Idle),
        (2, "b".to_owned(), State::Working),
    ];
    assert_eq!(sessions_to_stop(busy), Err("b".to_owned()));
}

#[test]
fn two_repositories_of_one_name_are_told_apart() {
    let mut spaces = recalled(1);
    let at = spaces
        .new_space("x", Member::at(Path::new("/a/app")), 1)
        .unwrap();
    spaces
        .add_member(at, Member::at(Path::new("/b/app")), 1)
        .unwrap();
    spaces.adopt(at);
    let names: Vec<String> = spaces
        .member_candidates()
        .iter()
        .map(|c| c.text().to_owned())
        .collect();
    assert_eq!(names, ["app", "app<2>"]);
    assert_eq!(spaces.member_named("app<2>"), Some(PathBuf::from("/b/app")));
}
