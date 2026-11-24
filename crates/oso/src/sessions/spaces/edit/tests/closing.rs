//! Closing a chosen space (ADR 0136, "A chosen space is closed, as amended"): the prompt's list and
//! what its answer names, a space not in front closed with the front left as it was, the refusals.
//!
//! Must not depend on: `app` or `dispatch`; these drive an editor alone, with no window.

use std::path::{Path, PathBuf};

use oso_core::editor::Editor;
use oso_core::frame::Way;
use oso_core::window::Window;

use super::{editor, id, named, no_session, recalled};
use crate::sessions::spaces::edit::{NotNamed, kept_open};
use crate::sessions::spaces::{Member, Stored};

/// Splits the selected window of the frame in two.
fn split(editor: &mut Editor) {
    editor.split_window(Way::Down, false).unwrap();
}

/// The texts of `candidates`, in order.
fn texts(candidates: &[oso_core::complete::Candidate]) -> Vec<String> {
    candidates.iter().map(|c| c.text().to_owned()).collect()
}

#[test]
fn closing_lists_the_open_spaces_the_one_in_front_first() {
    let mut editor = editor();
    let mut spaces = recalled(4);
    for (n, name) in ["d", "a", "c", "b"].into_iter().enumerate() {
        let root = format!("/{n}");
        spaces
            .new_space(&named(name), Member::at(Path::new(&root)), 1, &no_session)
            .unwrap();
    }
    assert_eq!(
        texts(&spaces.closing_candidates()),
        ["a", "b", "c", "d"],
        "none in front"
    );
    spaces.adopt(2);
    assert_eq!(texts(&spaces.closing_candidates()), ["c", "a", "b", "d"]);
    assert_eq!(texts(&spaces.candidates()), ["a", "b", "d", "c"]);
    spaces.enter(&mut editor, 0).unwrap();
    assert_eq!(texts(&spaces.closing_candidates()), ["d", "a", "b", "c"]);
    spaces.archive(&mut editor, 1, 2).unwrap();
    assert_eq!(
        texts(&spaces.closing_candidates()),
        ["d", "b", "c"],
        "a closed space is not offered"
    );
    assert_eq!(
        spaces.open_named("a"),
        Err(NotNamed::Nothing),
        "nor answered to"
    );
    assert_eq!(spaces.open_named("b"), Ok(3));
}

#[test]
fn an_answer_to_close_names_an_open_space_by_display_name_then_slug() {
    let mut spaces = recalled(0);
    let space = |n: u8, slug: &str, display: &str| {
        let root = format!("/{n}");
        let member = Member::at(Path::new(&root));
        let mut stored = Stored::of_repository(id(n), slug.into(), member, 1);
        stored.display = display.into();
        // With a directory, a slug stays, though another space has it (ADR 0159).
        stored.directory = Some(PathBuf::from(format!("/spaces/{n}")));
        stored
    };
    let stored = vec![
        space(1, "login-fix", "ログイン"),
        space(2, "twin", ""),
        space(3, "twin", ""),
    ];
    spaces.recall(stored, Vec::new(), false);
    assert_eq!(spaces.open_named("ログイン"), Ok(0));
    assert_eq!(spaces.open_named("login-fix"), Ok(0));
    // Two called so: neither is closed for the other, even with one in front.
    assert_eq!(spaces.open_named("twin"), Err(NotNamed::Several));
    spaces.adopt(2);
    assert_eq!(spaces.open_named("twin"), Err(NotNamed::Several));
    spaces.archive(&mut editor(), 1, 2).unwrap();
    assert_eq!(spaces.open_named("twin"), Ok(2));
}

#[test]
fn closing_a_space_not_in_front_leaves_the_front_and_its_windows() {
    let mut editor = editor();
    let mut spaces = recalled(3);
    let [a, b, c] = ["a", "b", "c"].map(|name| {
        let root = format!("/{name}");
        spaces
            .new_space(&named(name), Member::at(Path::new(&root)), 1, &no_session)
            .unwrap()
    });
    // Each with a tree of its own.
    let file = editor.window().buffer();
    for at in [b, c] {
        spaces.showed(at, None, Some(file));
    }
    spaces.adopt(a);
    spaces.enter(&mut editor, b).unwrap();
    // Two windows in front, and in the space to close, zoomed to one of its two.
    split(&mut editor);
    spaces.zoom(&mut editor).unwrap();
    spaces.enter(&mut editor, c).unwrap();
    split(&mut editor);
    let shown = |editor: &Editor| editor.windows().map(Window::id).collect::<Vec<_>>();
    let (windows, selected) = (shown(&editor), editor.window().id());
    spaces.archive(&mut editor, b, 2).unwrap();
    assert_eq!(spaces.current(), Some(c), "the front stays");
    assert_eq!(shown(&editor), windows);
    assert_eq!(editor.window().id(), selected);
    assert!(spaces.stored(b).unwrap().archived);
    // The front closed next: the closed one, entered after `a`, is passed over.
    spaces.archive(&mut editor, c, 3).unwrap();
    assert_eq!(spaces.current(), Some(a));
    // Reopened, it has its windows, and its zoom goes back out.
    spaces.reopen(b, 4);
    spaces.enter(&mut editor, b).unwrap();
    assert_eq!(editor.windows().count(), 1);
    spaces.zoom(&mut editor).unwrap();
    assert_eq!(editor.windows().count(), 2);
}

#[test]
fn a_prompt_refuses_the_close_of_any_space() {
    let mut editor = editor();
    let mut spaces = recalled(2);
    let [a, b] = ["a", "b"].map(|name| {
        let root = format!("/{name}");
        spaces
            .new_space(&named(name), Member::at(Path::new(&root)), 1, &no_session)
            .unwrap()
    });
    spaces.adopt(a);
    editor
        .open_prompt("Text:", oso_core::minibuffer::Kind::Text { history: None })
        .unwrap();
    for at in [a, b] {
        assert_eq!(
            spaces.archive(&mut editor, at, 2),
            Err(crate::sessions::spaces::Refused)
        );
        assert!(!spaces.stored(at).unwrap().archived);
    }
    assert_eq!(spaces.current(), Some(a));
}

#[test]
fn a_refused_close_names_the_session_and_its_space() {
    assert_eq!(
        kept_open("w1", "Fix login"),
        "Session w1 of space Fix login is working or needs input; the space stays open"
    );
}
