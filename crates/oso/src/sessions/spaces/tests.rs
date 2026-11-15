//! Spaces: each space's tree kept across switches, the first entry's tree, and zoom kept per
//! space; and which space a session joins (ADR 0136).
//!
//! Must not depend on: `app` or `dispatch`; these drive an editor alone, with no
//! window.

use std::path::{Path, PathBuf};

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;
use oso_core::frame::{FrameSize, Way};
use oso_core::selection::SelectionSet;
use oso_core::window::WindowId;

use super::{Member, Refused, SpaceId, Spaces, Stored, Zoom};

fn id(n: u8) -> SpaceId {
    SpaceId::from_random([n; 16])
}

/// A space of the one repository at `root`, recalled.
fn add(spaces: &mut Spaces, root: &str) -> usize {
    let n = u8::try_from(spaces.iter().count()).unwrap() + 1;
    let name = Path::new(root)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let stored = Stored::of_repository(id(n), name, Member::at(Path::new(root)), 1);
    spaces.recall(vec![stored], Vec::new(), false);
    spaces.iter().count() - 1
}

/// Spaces recalled with no space, and `ids` fresh ids.
fn recalled(ids: u8) -> Spaces {
    let mut spaces = Spaces::default();
    spaces.recall(Vec::new(), (100..100 + ids).map(id).collect(), false);
    spaces
}

fn editor() -> Editor {
    let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", &text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 120,
    });
    editor
}

fn ids(editor: &Editor) -> Vec<WindowId> {
    editor.frame().windows().iter().map(|(id, _)| *id).collect()
}

fn shown(editor: &Editor) -> Vec<String> {
    editor
        .frame()
        .windows()
        .iter()
        .map(|(id, _)| {
            let window = editor.window_by_id(*id).unwrap();
            editor.shown(window).name().to_owned()
        })
        .collect()
}

#[test]
fn each_space_keeps_its_tree_and_a_first_entry_shows_its_last_buffers() {
    let mut editor = editor();
    let mut spaces = Spaces::default();
    let one = add(&mut spaces, "/src/one");
    let two = add(&mut spaces, "/src/two");
    spaces.enter(&mut editor, one).unwrap();
    assert_eq!(
        spaces.current(),
        Some(one),
        "the frame's tree is the first space's"
    );
    editor.split_window(Way::Across, false).unwrap();
    let first_tree = ids(&editor);
    let file = editor.add_buffer(Buffer::from_text("b.rs", "b"));
    let session = editor.add_buffer(Buffer::from_text("*claude: two*", "c"));
    spaces.showed(two, Some(session), Some(file));
    spaces.enter(&mut editor, two).unwrap();
    assert_eq!(shown(&editor), ["b.rs", "*claude: two*"], "side by side");
    editor.split_window(Way::Down, false).unwrap();
    assert_eq!(ids(&editor).len(), 3);
    spaces.enter(&mut editor, one).unwrap();
    assert_eq!(ids(&editor), first_tree, "space one's tree, as it was");
    spaces.enter(&mut editor, two).unwrap();
    assert_eq!(ids(&editor).len(), 3, "space two's, with its split");
}

#[test]
fn a_space_with_nothing_shown_takes_the_frames_tree() {
    let mut editor = editor();
    let mut spaces = Spaces::default();
    let one = add(&mut spaces, "/src/one");
    let empty = add(&mut spaces, "/src/empty");
    spaces.adopt(one);
    let before = ids(&editor);
    spaces.enter(&mut editor, empty).unwrap();
    assert_eq!(ids(&editor), before);
    let file = editor.add_buffer(Buffer::from_text("one.rs", "1"));
    spaces.showed(one, None, Some(file));
    spaces.enter(&mut editor, one).unwrap();
    assert_eq!(
        shown(&editor),
        ["one.rs"],
        "built again from its last buffer"
    );
}

#[test]
fn a_prompt_open_refuses_and_changes_nothing() {
    let mut editor = editor();
    let mut spaces = Spaces::default();
    let one = add(&mut spaces, "/src/one");
    let two = add(&mut spaces, "/src/two");
    spaces.adopt(one);
    let file = editor.add_buffer(Buffer::from_text("b.rs", "b"));
    spaces.showed(two, None, Some(file));
    editor
        .open_completion("Space: ", "space", std::sync::Arc::from([]))
        .unwrap();
    let before = ids(&editor);
    assert_eq!(spaces.enter(&mut editor, two), Err(Refused));
    assert_eq!(spaces.zoom(&mut editor), Err(Refused));
    assert_eq!((ids(&editor), spaces.current()), (before, Some(one)));
}

#[test]
fn zoom_is_kept_per_space_and_back_keeps_the_zoomed_windows_place() {
    let mut editor = editor();
    let mut spaces = Spaces::default();
    let one = add(&mut spaces, "/src/one");
    let two = add(&mut spaces, "/src/two");
    spaces.adopt(one);
    editor.split_window(Way::Down, false).unwrap();
    let tree = ids(&editor);
    let from = editor.window().id();
    assert_eq!(spaces.zoom(&mut editor), Ok(Zoom::In));
    assert_eq!(ids(&editor).len(), 1);
    editor.current_mut().0.selections = SelectionSet::new(700);
    editor.follow_cursor();
    // Away and back: still zoomed.
    let file = editor.add_buffer(Buffer::from_text("b.rs", "b"));
    spaces.showed(two, None, Some(file));
    spaces.enter(&mut editor, two).unwrap();
    spaces.enter(&mut editor, one).unwrap();
    assert_eq!(ids(&editor).len(), 1);
    assert_eq!(spaces.zoom(&mut editor), Ok(Zoom::Out));
    assert_eq!(ids(&editor), tree);
    assert_eq!(editor.window().id(), from);
    assert_eq!(editor.window().selections.primary().cursor, 700);
}

#[test]
fn zoom_from_the_side_window_acts_on_the_tree() {
    let mut editor = editor();
    let list = editor.add_buffer(Buffer::from_text("*Sessions*", "s"));
    let side = editor.show_side_window(list, 32);
    editor.select_window(side);
    let mut spaces = Spaces::default();
    assert_eq!(spaces.zoom(&mut editor), Ok(Zoom::In));
    assert_ne!(editor.window().id(), side);
    assert_eq!(editor.side_window(), Some(side), "the side window stays");
    assert_eq!(spaces.zoom(&mut editor), Ok(Zoom::Out));
}

#[test]
fn a_zoom_goes_with_the_frames_tree() {
    let mut editor = editor();
    let mut spaces = Spaces::default();
    editor.split_window(Way::Down, false).unwrap();
    let tree = ids(&editor);
    // Before the first session: no space yet.
    assert_eq!(spaces.zoom(&mut editor), Ok(Zoom::In));
    let one = add(&mut spaces, "/src/one");
    spaces.enter(&mut editor, one).unwrap();
    assert_eq!(ids(&editor).len(), 1);
    // To a space with nothing to show, which takes the zoomed tree, and its zoom.
    let empty = add(&mut spaces, "/src/empty");
    spaces.enter(&mut editor, empty).unwrap();
    assert_eq!(ids(&editor).len(), 1);
    assert_eq!(spaces.zoom(&mut editor), Ok(Zoom::Out));
    assert_eq!(ids(&editor), tree);
}

#[test]
fn no_session_joins_before_the_spaces_are_recalled() {
    let mut spaces = Spaces::default();
    assert_eq!(spaces.join(Path::new("/src/oso"), None, 1), None);
    assert!(!spaces.wants_ids(), "nor are ids asked for");
}

#[test]
fn a_session_in_a_new_repository_makes_a_space_of_it_named_after_it() {
    let mut spaces = recalled(2);
    let at = spaces.join(Path::new("/src/oso"), None, 5).unwrap();
    let stored = &spaces.get(at).unwrap().stored;
    assert_eq!(stored.name, "oso");
    assert_eq!(stored.members, [Member::at(Path::new("/src/oso"))]);
    assert_eq!((stored.created, stored.directory.as_deref()), (5, None));
    let again = spaces.join(Path::new("/src/oso"), None, 6);
    assert_eq!(again, Some(at), "the same repository joins the same space");
    let other = spaces.join(Path::new("/elsewhere/oso"), None, 7).unwrap();
    assert_eq!(spaces.get(other).unwrap().name(), "oso-2");
    assert_eq!(
        spaces.join(Path::new("/src/third"), None, 8),
        None,
        "no id left"
    );
    assert!(spaces.wants_ids());
    assert!(!spaces.wants_ids(), "asked once");
    spaces.give_ids(vec![id(50)]);
    assert!(spaces.join(Path::new("/src/third"), None, 9).is_some());
}

#[test]
fn a_worktree_joins_the_space_of_its_repository_and_is_held_there() {
    let mut spaces = recalled(4);
    let repo = spaces.join(Path::new("/src/oso"), None, 1).unwrap();
    // Listed by git as a worktree of the repository.
    let elsewhere = Path::new("/wt/fix");
    assert_eq!(
        spaces.join(elsewhere, Some(Path::new("/src/oso")), 2),
        Some(repo)
    );
    // Inside the repository, as oso makes them, with no listing yet.
    let inside = Path::new("/src/oso/.claude/worktrees/x");
    assert_eq!(spaces.join(inside, None, 3), Some(repo));
    let held = &spaces.get(repo).unwrap().stored.worktrees;
    assert_eq!(held, &[PathBuf::from("/wt/fix"), PathBuf::from(inside)]);
    assert_eq!(spaces.holding(inside), Some(repo));
    assert_eq!(spaces.of_path(&inside.join("src/main.rs")), Some(repo));
    assert_eq!(spaces.of_path(Path::new("/tmp")), None);
}

#[test]
fn a_repository_in_several_spaces_joins_the_one_in_front_else_the_one_entered_last() {
    let mut editor = editor();
    let mut spaces = recalled(0);
    let a = add(&mut spaces, "/src/oso");
    let b = add(&mut spaces, "/src/oso");
    let c = add(&mut spaces, "/src/other");
    spaces.adopt(a);
    spaces.enter(&mut editor, b).unwrap();
    spaces.enter(&mut editor, c).unwrap();
    assert_eq!(
        spaces.join(Path::new("/src/oso"), None, 1),
        Some(b),
        "entered last"
    );
    spaces.enter(&mut editor, a).unwrap();
    assert_eq!(
        spaces.join(Path::new("/src/oso"), None, 1),
        Some(a),
        "in front"
    );
}

#[test]
fn a_worktree_is_held_by_one_space_and_forgotten_when_removed() {
    let mut spaces = recalled(0);
    let a = add(&mut spaces, "/src/oso");
    let b = add(&mut spaces, "/src/oso");
    let path = Path::new("/src/oso/.claude/worktrees/x");
    spaces.hold_worktree(a, path, 2);
    spaces.hold_worktree(b, path, 3);
    assert!(!spaces.get(a).unwrap().stored.holds_worktree(path));
    assert!(spaces.get(b).unwrap().stored.holds_worktree(path));
    assert_eq!(spaces.get(b).unwrap().stored.changed, 3);
    let (kept, known) = spaces.take_unsaved().expect("changed");
    assert_eq!(kept.len(), 2);
    assert_eq!(known.len(), 2);
    assert!(spaces.take_unsaved().is_none(), "written once");
    spaces.forget_worktree(path, 4);
    assert_eq!(spaces.holding(path), None);
    assert!(spaces.take_unsaved().is_some());
}

#[test]
fn space_switch_lists_open_spaces_with_their_title_or_repositories() {
    let mut spaces = recalled(0);
    let mut titled =
        Stored::of_repository(id(1), "fix-login".into(), Member::at(Path::new("/b")), 1);
    titled.title = "Login error".into();
    titled.members.push(Member::at(Path::new("/f")));
    let mut closed = Stored::of_repository(id(2), "old".into(), Member::at(Path::new("/o")), 1);
    closed.archived = true;
    let bare = Stored::of_repository(id(3), "app".into(), Member::at(Path::new("/src/app")), 1);
    spaces.recall(vec![titled, closed, bare], Vec::new(), true);
    let listed: Vec<(String, String)> = spaces
        .candidates()
        .iter()
        .map(|c| (c.text().to_owned(), c.annotation().to_owned()))
        .collect();
    assert_eq!(
        listed,
        [
            ("app".to_owned(), "app".to_owned()),
            ("fix-login".to_owned(), "Login error".to_owned())
        ]
    );
    assert!(
        spaces.take_unsaved().is_some(),
        "spaces made from sessions are kept at once"
    );
    assert_eq!(
        spaces.join(Path::new("/o"), None, 1),
        None,
        "an archived space takes none, and no id is left for a new one"
    );
}
