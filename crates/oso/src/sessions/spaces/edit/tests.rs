//! The space commands' changes: a new space named as its naming says, its repositories, a name
//! taken from its directory, renaming its display name, and archiving and reopening it, with
//! the space entered last taking the frame (ADR 0136, step 1b; ADR 0159).
//!
//! Must not depend on: `app` or `dispatch`; these drive an editor alone, with no window.

use std::path::{Path, PathBuf};

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;

use super::{Naming, NotAdded};
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

/// A naming whose slug and display name are both `text`.
fn named(text: &str) -> Naming {
    Naming {
        description: text.to_owned(),
        display: text.to_owned(),
        slug: text.to_owned(),
    }
}

/// No session's name.
fn no_session(_: &str) -> bool {
    false
}

fn editor() -> Editor {
    Editor::with_buffer(Buffer::from_text("a.rs", "a\n"))
}

#[test]
fn a_new_space_takes_its_naming_cleaned_and_unique() {
    let mut spaces = recalled(3);
    let typed = " Fix\u{200B} the  login\u{1b} error ";
    let at = spaces
        .new_space(&named(typed), Member::at(Path::new("/b")), 5, &no_session)
        .unwrap();
    let stored = spaces.stored(at).unwrap();
    assert_eq!(stored.description, "Fix the login error");
    assert_eq!(stored.name, "fix-the-login-error");
    assert_eq!(stored.display, "Fix the login error");
    assert_eq!(stored.members, [Member::at(Path::new("/b"))]);
    let again = spaces
        .new_space(
            &named("Fix the login error"),
            Member::at(Path::new("/c")),
            6,
            &no_session,
        )
        .unwrap();
    let stored = spaces.stored(again).unwrap();
    assert_eq!(stored.name, "fix-the-login-error-2");
    assert_eq!(stored.display, "Fix the login error 2", "no two read alike");
    let japanese = Naming {
        description: "ログインの修正".into(),
        display: "ログイン 🔧".into(),
        slug: "login-fix".into(),
    };
    let at = spaces
        .new_space(&japanese, Member::at(Path::new("/d")), 7, &no_session)
        .unwrap();
    assert_eq!(spaces.stored(at).unwrap().label(), "ログイン 🔧");
    assert_eq!(spaces.stored(at).unwrap().name, "login-fix");
    assert_eq!(
        spaces.new_space(&named("more"), Member::at(Path::new("/e")), 8, &no_session),
        None,
        "no id left"
    );
}

#[test]
fn a_new_spaces_slug_is_no_sessions_name_nor_an_earlier_orchestrators() {
    let mut spaces = recalled(3);
    let worker = |name: &str| name.eq_ignore_ascii_case("api");
    let at = spaces
        .new_space(&named("api"), Member::at(Path::new("/a")), 1, &worker)
        .unwrap();
    assert_eq!(spaces.stored(at).unwrap().name, "api-2");
    spaces
        .new_space(&named("oso"), Member::at(Path::new("/b")), 1, &no_session)
        .unwrap();
    let at = spaces
        .new_space(
            &named("oso-was-1"),
            Member::at(Path::new("/c")),
            1,
            &no_session,
        )
        .unwrap();
    assert_eq!(
        spaces.stored(at).unwrap().name,
        "oso-was-1-2",
        "the name space oso's first earlier orchestrator keeps"
    );
}

#[test]
fn repositories_are_added_once_up_to_the_most_and_taken_out() {
    let mut spaces = recalled(1);
    let at = spaces
        .new_space(&named("x"), Member::at(Path::new("/r0")), 1, &no_session)
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
    let mut spaces = recalled(3);
    let a = spaces
        .new_space(&named("oso"), Member::at(Path::new("/a")), 1, &no_session)
        .unwrap();
    let b = spaces
        .new_space(&named("notes"), Member::at(Path::new("/b")), 1, &no_session)
        .unwrap();
    let c = spaces
        .new_space(&named("api"), Member::at(Path::new("/c")), 1, &no_session)
        .unwrap();
    let path = PathBuf::from("/s/oso-2");
    spaces.set_directory(a, path, Some((3, 4)), ("oso-2", "oso"), &no_session, 2);
    let stored = spaces.stored(a).unwrap();
    assert_eq!(stored.name, "oso-2");
    assert_eq!(stored.directory.as_deref(), Some(Path::new("/s/oso-2")));
    assert_eq!(stored.directory_identity, Some((3, 4)));
    // A name another space has is not taken, nor a session's.
    let path = PathBuf::from("/s/oso-2x");
    spaces.set_directory(b, path, None, ("oso-2", "notes"), &no_session, 3);
    assert_eq!(spaces.stored(b).unwrap().name, "notes");
    let worker = |name: &str| name == "api-2";
    let path = PathBuf::from("/s/api-2");
    spaces.set_directory(c, path, None, ("api-2", "api"), &worker, 3);
    assert_eq!(spaces.stored(c).unwrap().name, "api");
}

#[test]
fn renaming_changes_the_display_name_alone_and_refuses_an_empty_or_taken_one() {
    let mut spaces = recalled(2);
    let a = spaces
        .new_space(&named("one"), Member::at(Path::new("/a")), 1, &no_session)
        .unwrap();
    spaces
        .new_space(&named("two"), Member::at(Path::new("/b")), 1, &no_session)
        .unwrap();
    assert_eq!(
        spaces.rename(a, " ログイン\u{202E}修正 👨\u{200D}👩\u{200D}👧 ", 2),
        Ok("ログイン修正 👨\u{200D}👩\u{200D}👧".to_owned())
    );
    let stored = spaces.stored(a).unwrap();
    assert_eq!(stored.name, "one", "the slug stays");
    assert_eq!(stored.label(), "ログイン修正 👨\u{200D}👩\u{200D}👧");
    assert!(spaces.rename(a, "\u{200B}", 3).is_err());
    assert!(
        spaces.rename(a, "TWO", 3).is_err(),
        "another space reads so"
    );
    assert_eq!(spaces.rename(a, "one", 4), Ok("one".to_owned()));
    assert_eq!(spaces.stored(a).unwrap().display, "", "its slug again");
    assert_eq!(spaces.named("one"), Some(a));
}

#[test]
fn closing_the_space_in_front_enters_the_one_entered_last_and_reopening_brings_it_back() {
    let mut editor = editor();
    let mut spaces = recalled(3);
    let a = spaces
        .new_space(&named("a"), Member::at(Path::new("/a")), 1, &no_session)
        .unwrap();
    let b = spaces
        .new_space(&named("b"), Member::at(Path::new("/b")), 1, &no_session)
        .unwrap();
    let c = spaces
        .new_space(&named("c"), Member::at(Path::new("/c")), 1, &no_session)
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
        .new_space(&named("fix"), Member::at(Path::new("/b")), 3, &no_session)
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
        .new_space(
            &named("x"),
            Member::at(Path::new("/src/back")),
            1,
            &no_session,
        )
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
fn a_space_renamed_while_its_directory_was_made_keeps_its_display_name() {
    let mut spaces = recalled(1);
    let at = spaces
        .new_space(&named("oso"), Member::at(Path::new("/a")), 1, &no_session)
        .unwrap();
    spaces.rename(at, "Other", 2).unwrap();
    let path = PathBuf::from("/s/oso-2");
    spaces.set_directory(at, path, None, ("oso-2", "oso"), &no_session, 3);
    let stored = spaces.stored(at).unwrap();
    assert_eq!((stored.name.as_str(), stored.label()), ("oso-2", "Other"));
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
        .new_space(&named("x"), Member::at(Path::new("/a/app")), 1, &no_session)
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
