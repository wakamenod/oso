//! The side window on the editor (ADR 0121): its cells, and what the tree's commands do
//! around it.
//!
//! Must not depend on: the host's window.

use crate::buffer::{Buffer, BufferId};
use crate::editor::Editor;
use crate::frame::{FrameSize, Side, Way};
use crate::window::WindowId;

fn editor(columns: usize) -> (Editor, BufferId) {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", "one\ntwo\n"));
    editor.set_frame_size(FrameSize { rows: 24, columns });
    let list = editor.add_buffer(Buffer::from_text("list", "a\nb\n"));
    (editor, list)
}

fn selected(editor: &Editor) -> WindowId {
    editor.window().id()
}

fn tree(editor: &Editor) -> Vec<WindowId> {
    editor.splits.iter().collect()
}

#[test]
fn it_takes_the_left_columns_and_a_divider_outside_the_tree() {
    let (mut editor, list) = editor(100);
    let first = selected(&editor);
    let side = editor.show_side_window(list, 32);
    assert_eq!(editor.side_window(), Some(side));
    assert_eq!(selected(&editor), first, "showing it selects nothing");
    let regions = *editor.regions(side).unwrap();
    assert_eq!((regions.text.column, regions.text.columns), (0, 32));
    assert_eq!(regions.divider.map(|d| d.column), Some(32));
    let tree_regions = *editor.regions(first).unwrap();
    assert_eq!(
        (tree_regions.text.column, tree_regions.text.columns),
        (33, 67)
    );
    assert_eq!(editor.frame().windows()[0].0, side, "listed first");
    assert_eq!(tree(&editor), [first], "not in the tree");
    assert_eq!(editor.tiling(), Editor::default().tiling());
    assert_eq!(editor.window_by_id(side).unwrap().buffer(), list);
}

#[test]
fn a_narrow_frame_gives_it_no_cells_and_never_selects_it() {
    let (mut editor, list) = editor(40);
    let first = selected(&editor);
    let side = editor.show_side_window(list, 32);
    let regions = *editor.regions(side).unwrap();
    assert_eq!(regions.text.columns + regions.modeline.columns, 0);
    assert_eq!(editor.regions(first).unwrap().text.columns, 40);
    editor.select_window(side);
    assert_eq!(selected(&editor), first);
}

#[test]
fn a_frame_that_narrows_under_the_selected_side_window_selects_the_tree() {
    let (mut editor, list) = editor(100);
    let first = selected(&editor);
    let side = editor.show_side_window(list, 32);
    editor.select_window(side);
    assert_eq!(selected(&editor), side);
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 30,
    });
    assert_eq!(selected(&editor), first);
}

#[test]
fn other_window_skips_it_and_windmove_and_select_reach_it() {
    let (mut editor, list) = editor(120);
    let first = selected(&editor);
    let second = editor.split_window(Way::Across, false).unwrap();
    let side = editor.show_side_window(list, 32);
    editor.other_window(1);
    assert_eq!(selected(&editor), second);
    editor.other_window(1);
    assert_eq!(selected(&editor), first);
    editor.windmove(Side::Left).unwrap();
    assert_eq!(selected(&editor), side);
    // From it, `C-x o` goes back to the tree's window selected last.
    editor.other_window(1);
    assert_eq!(selected(&editor), first);
    editor.select_window(side);
    editor.windmove(Side::Right).unwrap();
    assert_eq!(selected(&editor), first);
}

#[test]
fn delete_other_windows_keeps_it_and_delete_window_in_it_hides_it() {
    let (mut editor, list) = editor(120);
    let first = selected(&editor);
    editor.split_window(Way::Across, false).unwrap();
    let side = editor.show_side_window(list, 32);
    assert!(editor.delete_other_windows());
    assert_eq!(tree(&editor), [first]);
    assert_eq!(editor.side_window(), Some(side));
    editor.select_window(side);
    editor.delete_window().unwrap();
    assert_eq!(editor.side_window(), None);
    assert!(editor.window_by_id(side).is_none());
    assert_eq!(selected(&editor), first);
    assert_eq!(editor.regions(first).unwrap().text.columns, 120);
}

#[test]
fn delete_other_windows_in_it_keeps_the_tree_window_selected_last() {
    let (mut editor, list) = editor(120);
    let second = editor.split_window(Way::Across, true).unwrap();
    let side = editor.show_side_window(list, 32);
    editor.select_window(side);
    assert!(editor.delete_other_windows());
    assert_eq!(tree(&editor), [second]);
    assert_eq!(selected(&editor), second);
    assert_eq!(editor.side_window(), Some(side));
}

#[test]
fn a_split_a_resize_and_show_in_it_act_on_the_tree_window_selected_last() {
    let (mut editor, list) = editor(120);
    let first = selected(&editor);
    let side = editor.show_side_window(list, 32);
    editor.select_window(side);
    let new = editor.split_window(Way::Down, false).unwrap();
    assert_eq!(tree(&editor), [first, new]);
    assert_eq!(selected(&editor), first);
    editor.select_window(side);
    editor.resize_window(Way::Down, 2).unwrap();
    assert_eq!(selected(&editor), first);
    assert_eq!(editor.regions(side).unwrap().text.columns, 32);
    editor.select_window(side);
    let other = editor.add_buffer(Buffer::from_text("other", "x"));
    editor.show(other);
    assert_eq!(selected(&editor), first);
    assert_eq!(editor.window().buffer(), other);
    assert_eq!(editor.window_by_id(side).unwrap().buffer(), list);
}

#[test]
fn showing_it_again_reuses_it_and_hiding_it_twice_does_nothing() {
    let (mut editor, list) = editor(120);
    let side = editor.show_side_window(list, 32);
    let other = editor.add_buffer(Buffer::from_text("other", "x"));
    assert_eq!(editor.show_side_window(other, 40), side);
    assert_eq!(editor.window_by_id(side).unwrap().buffer(), other);
    assert_eq!(editor.regions(side).unwrap().text.columns, 40);
    assert!(editor.hide_side_window());
    assert!(!editor.hide_side_window());
    let gone = BufferId(9_999);
    let first = selected(&editor);
    assert_eq!(editor.show_side_window(gone, 32), first);
    assert_eq!(editor.side_window(), None);
}

#[test]
fn the_wheel_and_a_click_reach_it() {
    let (mut editor, list) = editor(120);
    let side = editor.show_side_window(list, 32);
    assert_eq!(editor.wheel_window(Some((3, 5))), side);
    assert_eq!(editor.tiled_window_index(side), editor.window_index(side));
}
