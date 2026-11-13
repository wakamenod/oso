//! Spaces: names and nesting, each space's tree kept across switches, the first entry's tree,
//! and zoom kept per space.
//!
//! Must not depend on: `app` or `dispatch`; these drive an editor alone, with no
//! window.

use std::path::Path;

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;
use oso_core::frame::{FrameSize, Way};
use oso_core::selection::SelectionSet;
use oso_core::window::WindowId;

use super::{Refused, Spaces, Zoom};

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
fn a_worktree_nests_under_its_repository_and_names_stay_apart() {
    let mut spaces = Spaces::default();
    let repo = spaces.add(Path::new("/src/oso"), None);
    let tree = spaces.add(Path::new("/src/oso-wt/fix-1"), Some(Path::new("/src/oso")));
    let other = spaces.add(Path::new("/elsewhere/oso"), None);
    assert_eq!(spaces.add(Path::new("/src/oso"), None), repo, "found again");
    let names: Vec<&str> = [repo, tree, other]
        .map(|at| spaces.get(at).unwrap().name.as_str())
        .to_vec();
    assert_eq!(names, ["oso", "oso/fix-1", "oso<2>"]);
    assert_eq!(
        spaces.get(tree).unwrap().repo.as_deref(),
        Some(Path::new("/src/oso"))
    );
    let texts: Vec<String> = spaces
        .candidates()
        .iter()
        .map(|c| c.text().to_owned())
        .collect();
    assert_eq!(texts, ["oso", "oso/fix-1", "oso<2>"]);
    assert_eq!(spaces.named("oso/fix-1"), Some(tree));
    assert_eq!(
        spaces.of_path(Path::new("/src/oso-wt/fix-1/src/main.rs")),
        Some(tree)
    );
    assert_eq!(spaces.of_path(Path::new("/src/oso/x")), Some(repo));
    assert_eq!(spaces.of_path(Path::new("/tmp")), None);
}

#[test]
fn each_space_keeps_its_tree_and_a_first_entry_shows_its_last_buffers() {
    let mut editor = editor();
    let mut spaces = Spaces::default();
    let one = spaces.add(Path::new("/src/one"), None);
    let two = spaces.add(Path::new("/src/two"), None);
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
    let space = spaces.get_mut(two).unwrap();
    space.last_file = Some(file);
    space.last_session = Some(session);
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
    let one = spaces.add(Path::new("/src/one"), None);
    let empty = spaces.add(Path::new("/src/empty"), None);
    spaces.adopt(one);
    let before = ids(&editor);
    spaces.enter(&mut editor, empty).unwrap();
    assert_eq!(ids(&editor), before);
    let file = editor.add_buffer(Buffer::from_text("one.rs", "1"));
    spaces.get_mut(one).unwrap().last_file = Some(file);
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
    let one = spaces.add(Path::new("/src/one"), None);
    let two = spaces.add(Path::new("/src/two"), None);
    spaces.adopt(one);
    let file = editor.add_buffer(Buffer::from_text("b.rs", "b"));
    spaces.get_mut(two).unwrap().last_file = Some(file);
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
    let one = spaces.add(Path::new("/src/one"), None);
    let two = spaces.add(Path::new("/src/two"), None);
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
    spaces.get_mut(two).unwrap().last_file = Some(file);
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
