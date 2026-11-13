//! Window trees swapped whole, for spaces and zoom (ADR 0121).
//!
//! Must not depend on: the host's window.

use crate::buffer::{Buffer, BufferId, Replacement};
use crate::editor::Editor;
use crate::editor::pairs::tests::{lines, pair};
use crate::frame::{FrameSize, Tiling, Way};
use crate::minibuffer::Kind;
use crate::selection::SelectionSet;
use crate::window::{RowMap, WindowId};

fn editor() -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(200)));
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 120,
    });
    editor
}

fn selected(editor: &Editor) -> WindowId {
    editor.window().id()
}

fn tree(editor: &Editor) -> Vec<WindowId> {
    editor.splits.iter().collect()
}

fn delete_text(editor: &mut Editor, id: BufferId, range: std::ops::Range<usize>) {
    let edit = Replacement { range, text: "" };
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit]).unwrap();
    editor.end_undo_group();
}

#[test]
fn a_swap_keeps_each_windows_cursors_and_view_and_the_selection() {
    let mut editor = editor();
    let first = selected(&editor);
    let second = editor.split_window(Way::Across, true).unwrap();
    editor.current_mut().0.selections = SelectionSet::new(900);
    editor.follow_cursor();
    let top = editor.window().scroll();
    let other = editor.add_buffer(Buffer::from_text("other", "x\n"));
    let fresh = editor.window_tree_showing(&[other]);
    let saved = editor.swap_window_tree(fresh).unwrap();
    assert_eq!(tree(&editor).len(), 1);
    assert_eq!(editor.window().buffer(), other);
    assert!(editor.window_by_id(first).is_none());
    assert_eq!(
        saved.tiling(),
        Tiling::Run {
            way: Way::Across,
            children: vec![(60, Tiling::Window), (60, Tiling::Window)],
        }
    );
    let back = editor.swap_window_tree(saved).unwrap();
    assert_eq!(back.buffers().collect::<Vec<_>>(), [other]);
    assert_eq!(tree(&editor), [first, second]);
    assert_eq!(selected(&editor), second, "the window selected last");
    assert_eq!(editor.window().selections.primary().cursor, 900);
    assert_eq!(editor.window().scroll(), top);
}

#[test]
fn a_buffer_killed_while_out_is_replaced_and_cursors_are_clipped() {
    let mut editor = editor();
    let kept = editor.window().buffer();
    let doomed = editor.add_buffer(Buffer::from_text("doomed", "abc\n"));
    let first = selected(&editor);
    editor.split_window(Way::Across, true).unwrap();
    editor.show(doomed);
    editor.select_window(first);
    editor.current_mut().0.selections = SelectionSet::new(1_500);
    let scratch = editor.add_buffer(Buffer::from_text("s", ""));
    let fresh = editor.window_tree_showing(&[scratch]);
    let saved = editor.swap_window_tree(fresh).unwrap();
    editor.kill_buffer(doomed);
    delete_text(&mut editor, kept, 100..1_800);
    editor.swap_window_tree(saved).unwrap();
    let buffers: Vec<BufferId> = editor
        .splits
        .iter()
        .map(|id| editor.window_by_id(id).unwrap().buffer())
        .collect();
    assert_eq!(buffers, [kept, kept], "back to what it showed before");
    let cursor = editor
        .window_by_id(first)
        .unwrap()
        .selections
        .primary()
        .cursor;
    assert!(
        cursor <= editor.buffer(kept).unwrap().text().len(),
        "{cursor}"
    );
}

#[test]
fn an_open_prompt_refuses_and_gives_the_tree_back() {
    let mut editor = editor();
    let lone = editor.lone_window_tree();
    let id = lone.splits.iter().next().unwrap();
    editor
        .open_prompt("Name: ", Kind::Text { history: None })
        .unwrap();
    let refused = editor.swap_window_tree(lone).unwrap_err();
    assert_eq!(refused.splits.iter().collect::<Vec<_>>(), [id]);
}

#[test]
fn a_new_tree_shows_one_or_two_buffers_or_one_like_the_selected_window() {
    let mut editor = editor();
    let shown = editor.window().buffer();
    let a = editor.add_buffer(Buffer::from_text("a", "a"));
    let b = editor.add_buffer(Buffer::from_text("b", "b"));
    let c = editor.add_buffer(Buffer::from_text("c", "c"));
    let both = editor.window_tree_showing(&[a, b]);
    assert_eq!(both.buffers().collect::<Vec<_>>(), [a, b]);
    assert_eq!(
        both.recent[0],
        both.splits.iter().next().unwrap(),
        "the first selected"
    );
    for buffers in [&[][..], &[a, b, c], &[BufferId(9_999)]] {
        let tree = editor.window_tree_showing(buffers);
        assert_eq!(tree.buffers().collect::<Vec<_>>(), [shown], "{buffers:?}");
    }
    let one = editor.window_tree_showing(&[c]);
    assert_eq!(one.buffers().collect::<Vec<_>>(), [c]);
    editor.swap_window_tree(both).unwrap();
    assert_eq!(editor.window().buffer(), a);
    let [left, right] = [0, 1].map(|i| *editor.regions(tree(&editor)[i]).unwrap());
    assert_eq!(left.outer().columns, 60);
    assert_eq!(right.outer().columns, 60);
}

#[test]
fn zoom_and_back_keeps_the_zoomed_windows_place() {
    let mut editor = editor();
    let first = selected(&editor);
    editor.split_window(Way::Down, false).unwrap();
    editor.current_mut().0.selections = SelectionSet::new(450);
    editor.follow_cursor();
    let lone = editor.lone_window_tree();
    let saved = editor.swap_window_tree(lone).unwrap();
    assert_eq!(tree(&editor).len(), 1);
    assert_eq!(editor.window().selections.primary().cursor, 450);
    assert_eq!(editor.regions(selected(&editor)).unwrap().text.rows, 22);
    editor.current_mut().0.selections = SelectionSet::new(700);
    let zoomed = editor.window().selections.clone();
    let scroll = editor.window().scroll();
    let buffer = editor.window().buffer();
    editor.swap_window_tree(saved).unwrap();
    assert_eq!(selected(&editor), first);
    editor.show_in(first, buffer);
    editor.restore_positions_in(first, zoomed, scroll);
    assert_eq!(editor.window().selections.primary().cursor, 700);
}

#[test]
fn a_pair_moves_with_its_tree_and_the_side_window_stays() {
    let (mut editor, first, second) = pair("x\n", RowMap::same());
    let list = editor.add_buffer(Buffer::from_text("list", "l"));
    let side = editor.show_side_window(list, 32);
    editor.select_window(side);
    let lone = editor.window_tree_showing(&[list]);
    let saved = editor.swap_window_tree(lone).unwrap();
    assert_eq!(selected(&editor), side, "the side window stays selected");
    assert_eq!(editor.pair_of(first), None);
    assert_eq!(editor.side_window(), Some(side));
    editor.swap_window_tree(saved).unwrap();
    assert_eq!(editor.pair_of(first), Some(second));
    assert_eq!(tree(&editor), [first, second]);
    assert_eq!(selected(&editor), side);
    editor.other_window(1);
    assert!([first, second].contains(&selected(&editor)));
}

#[test]
fn a_secret_buffer_in_a_swapped_tree_still_withholds() {
    let mut editor = editor();
    let mut secret = Buffer::from_text(".env", "TOKEN=x\n");
    secret.treat_as_secret();
    let secret = editor.add_buffer(secret);
    editor.show(secret);
    let plain = editor.add_buffer(Buffer::from_text("p", "p"));
    let fresh = editor.window_tree_showing(&[plain]);
    let saved = editor.swap_window_tree(fresh).unwrap();
    assert!(!editor.shows_secret());
    editor.swap_window_tree(saved).unwrap();
    assert!(editor.shows_secret());
}
