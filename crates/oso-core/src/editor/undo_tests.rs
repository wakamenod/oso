//! Tests of undo through [`Editor`]: groups, joining, the modified flag and the
//! byte bound (ADR 0053).
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written
//! out here.

use super::edit_tests::{cursors, edit, editor_with, plain_edit, second_window, self_insert, text};
use super::*;
use crate::display_map::TabWidth;
use crate::selection::Selection;

#[test]
fn undoing_to_the_saved_state_is_unmodified_and_redoing_is_modified() {
    let (mut editor, id) = editor_with("abc");
    assert!(!editor.buffer(id).unwrap().is_modified());
    plain_edit(&mut editor, id, 0..0, "x");
    assert!(editor.buffer(id).unwrap().is_modified());
    let saved = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(saved);
    assert!(!editor.buffer(id).unwrap().is_modified());

    plain_edit(&mut editor, id, 0..0, "y");
    assert!(editor.buffer(id).unwrap().is_modified());
    editor.undo().unwrap();
    assert!(!editor.buffer(id).unwrap().is_modified(), "back at saved");
    assert_eq!(editor.buffer(id).unwrap().content_id(), saved);
    editor.undo().unwrap();
    assert!(editor.buffer(id).unwrap().is_modified(), "before saved");
    editor.redo().unwrap();
    assert!(!editor.buffer(id).unwrap().is_modified());
    editor.redo().unwrap();
    assert!(editor.buffer(id).unwrap().is_modified());
}

#[test]
fn the_same_text_by_another_route_is_still_modified() {
    let (mut editor, id) = editor_with("abc");
    plain_edit(&mut editor, id, 0..0, "x");
    plain_edit(&mut editor, id, 0..1, "");
    assert_eq!(text(&editor, id), "abc");
    assert!(editor.buffer(id).unwrap().is_modified());
}

#[test]
fn an_edit_after_an_undo_discards_the_redo_and_saved_past_it_stays_modified() {
    let (mut editor, id) = editor_with("abc");
    plain_edit(&mut editor, id, 0..0, "x");
    let saved = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(saved);
    editor.undo().unwrap();
    plain_edit(&mut editor, id, 0..0, "y");
    assert_eq!(editor.redo(), Err(EditError::NoRedo));
    assert_eq!(text(&editor, id), "yabc");
    assert!(editor.buffer(id).unwrap().is_modified());
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc");
    assert!(
        editor.buffer(id).unwrap().is_modified(),
        "the saved state cannot be reached"
    );
}

#[test]
fn undo_and_redo_with_nothing_left_are_errors_and_change_nothing() {
    let (mut editor, id) = editor_with("abc");
    let revision = editor.buffer(id).unwrap().revision();
    assert_eq!(editor.undo(), Err(EditError::NoUndo));
    assert_eq!(editor.redo(), Err(EditError::NoRedo));
    assert_eq!(editor.buffer(id).unwrap().revision(), revision);
}

#[test]
fn undo_bumps_the_revision() {
    let (mut editor, id) = editor_with("abc");
    plain_edit(&mut editor, id, 0..0, "x");
    let revision = editor.buffer(id).unwrap().revision();
    editor.undo().unwrap();
    assert_eq!(editor.buffer(id).unwrap().revision(), revision + 1);
}

#[test]
fn undo_restores_cursors_here_and_maps_them_elsewhere() {
    let (mut editor, id) = editor_with("0123456789");
    editor.windows[0].selections = SelectionSet::new(3);
    let other = second_window(&mut editor, id, &[8]);
    plain_edit(&mut editor, id, 3..5, "");
    assert_eq!(cursors(&editor.windows[other]), [6]);
    editor.windows[0].selections = SelectionSet::new(0);
    editor.undo().unwrap();
    assert_eq!(cursors(&editor.windows[0]), [3], "from before the group");
    assert_eq!(cursors(&editor.windows[other]), [8], "follows the text");
    editor.windows[0].selections = SelectionSet::new(0);
    editor.redo().unwrap();
    assert_eq!(cursors(&editor.windows[0]), [3], "from after the group");
    assert_eq!(cursors(&editor.windows[other]), [6]);
}

#[test]
fn a_group_holds_a_command_and_the_cursors_at_its_edges() {
    let (mut editor, id) = editor_with("0123456789");
    editor.windows[0].selections = SelectionSet::new(9);
    editor.begin_undo_group();
    // Motion before the edit is part of the command.
    editor.windows[0].selections = SelectionSet::new(1);
    editor.apply_edits(id, &[edit(1..1, "a")]).unwrap();
    editor.apply_edits(id, &[edit(5..5, "b")]).unwrap();
    // A command may place the cursors after its edits, as `open-line` does.
    editor.windows[0].selections = SelectionSet::new(4);
    editor.end_undo_group();
    assert_eq!(text(&editor, id), "0a123b456789");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));

    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "0123456789");
    assert_eq!(cursors(&editor.windows[0]), [9]);
    editor.redo().unwrap();
    assert_eq!(text(&editor, id), "0a123b456789");
    assert_eq!(cursors(&editor.windows[0]), [4]);
}

#[test]
fn without_a_group_each_edit_is_a_step() {
    let (mut editor, id) = editor_with("");
    editor.apply_edits(id, &[edit(0..0, "a")]).unwrap();
    editor.apply_edits(id, &[edit(1..1, "b")]).unwrap();
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "a");
    assert_eq!(cursors(&editor.windows[0]), [1]);
}

#[test]
fn a_group_left_open_closes_when_the_next_begins() {
    let (mut editor, id) = editor_with("");
    editor.begin_undo_group();
    editor.allow_undo_join();
    editor.apply_edits(id, &[edit(0..0, "a")]).unwrap();
    editor.windows[0].selections = SelectionSet::new(0);
    // A path that missed its end. It ended with the cursors as they were.
    self_insert(&mut editor, "b");
    assert_eq!(text(&editor, id), "ba");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0), "no joining");
    editor.end_undo_group();
    assert!(editor.undo_scope.is_none());
    editor.undo().unwrap();
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "");
    editor.redo().unwrap();
    assert_eq!(cursors(&editor.windows[0]), [0], "as the open group ended");
}

#[test]
fn the_panic_path_closes_the_group_and_keeps_what_was_applied() {
    let (mut editor, id) = editor_with("abc");
    editor.begin_undo_group();
    editor.allow_undo_join();
    editor.apply_edits(id, &[edit(0..0, "x")]).unwrap();
    editor.end_undo_group_after_panic();
    assert!(editor.undo_scope.is_none());
    assert_eq!(text(&editor, id), "xabc");
    assert_eq!(cursors(&editor.windows[0]), [1]);
    editor.end_undo_group_after_panic();

    // Nothing joins it, even a command that would.
    self_insert(&mut editor, "y");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "xabc");
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc");
}

#[test]
fn a_panic_inside_a_command_leaves_buffer_and_history_in_step() {
    let (mut editor, id) = editor_with("abc");
    editor.begin_undo_group();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        editor.apply_edits(id, &[edit(0..0, "x")]).unwrap();
        let _staged = editor.buffer(id).unwrap().stage_edits(&[edit(0..0, "y")]);
        panic!("a bug in a command");
    }));
    assert!(result.is_err());
    editor.end_undo_group_after_panic();
    assert_eq!(text(&editor, id), "xabc", "a stage that was not committed");
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc");
    assert!(!editor.buffer(id).unwrap().is_modified());
}

#[test]
fn consecutive_self_inserts_join_one_group() {
    let (mut editor, id) = editor_with("");
    self_insert(&mut editor, "a");
    self_insert(&mut editor, "b");
    self_insert(&mut editor, "c");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "");
    editor.redo().unwrap();
    assert_eq!(text(&editor, id), "abc");
    assert_eq!(cursors(&editor.windows[0]), [3]);
}

#[test]
fn a_command_in_between_ends_the_joining_even_with_the_cursors_back() {
    let (mut editor, id) = editor_with("abc");
    self_insert(&mut editor, "x");
    // A motion and its reverse: cursors as the group left them.
    editor.begin_undo_group();
    editor.end_undo_group();
    self_insert(&mut editor, "y");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
}

#[test]
fn a_command_that_does_not_allow_joining_starts_a_group() {
    let (mut editor, id) = editor_with("abc");
    self_insert(&mut editor, "x");
    plain_edit(&mut editor, id, 1..1, "y");
    assert_eq!(cursors(&editor.windows[0]), [2]);
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
}

#[test]
fn an_undo_and_redo_outside_a_command_end_the_joining() {
    let (mut editor, id) = editor_with("abc");
    self_insert(&mut editor, "x");
    editor.undo().unwrap();
    editor.redo().unwrap();
    self_insert(&mut editor, "y");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
}

#[test]
fn moved_cursors_end_the_joining() {
    let (mut editor, id) = editor_with("abc");
    self_insert(&mut editor, "x");
    editor.windows[0].selections = SelectionSet::new(3);
    self_insert(&mut editor, "y");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
}

#[test]
fn an_undo_ends_the_joining() {
    let (mut editor, id) = editor_with("abc");
    self_insert(&mut editor, "x");
    self_insert(&mut editor, "y");
    editor.undo().unwrap();
    self_insert(&mut editor, "z");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
    assert_eq!(text(&editor, id), "zabc");
    editor.undo().unwrap();
    assert_eq!(editor.redo(), Ok(()));
    assert_eq!(editor.redo(), Err(EditError::NoRedo));
}

#[test]
fn a_joining_command_that_edited_nothing_does_not_rearm_joining() {
    let (mut editor, id) = editor_with("abc");
    self_insert(&mut editor, "x");
    // A motion and its reverse, then a self-insert or IME commit of nothing.
    editor.begin_undo_group();
    editor.end_undo_group();
    self_insert(&mut editor, "");
    self_insert(&mut editor, "y");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "xabc", "one undo removes only the y");
}

#[test]
fn a_joining_command_that_undoes_does_not_rearm_joining() {
    let (mut editor, id) = editor_with("abc");
    self_insert(&mut editor, "x");
    plain_edit(&mut editor, id, 1..1, "y");
    // Back to where the group of x ended, in a command that allows joining.
    editor.begin_undo_group();
    editor.allow_undo_join();
    editor.undo().unwrap();
    editor.end_undo_group();
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 1));
    editor.redo().unwrap();
    editor.undo().unwrap();
    self_insert(&mut editor, "z");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
}

#[test]
fn a_joining_command_that_undoes_and_edits_does_not_rearm_joining() {
    let (mut editor, id) = editor_with("abc");
    plain_edit(&mut editor, id, 0..0, "x");
    editor.begin_undo_group();
    editor.allow_undo_join();
    editor.undo().unwrap();
    editor.apply_edits(id, &[edit(0..0, "y")]).unwrap();
    editor.end_undo_group();
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
    self_insert(&mut editor, "z");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
}

#[test]
fn an_edit_outside_a_command_ends_the_joining() {
    let (mut editor, id) = editor_with("");
    self_insert(&mut editor, "a");
    editor.apply_edits(id, &[edit(1..1, "b")]).unwrap();
    self_insert(&mut editor, "c");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (3, 0));
}

#[test]
fn joining_outside_a_group_does_nothing() {
    let (mut editor, id) = editor_with("");
    editor.allow_undo_join();
    assert!(editor.undo_scope.is_none());
    self_insert(&mut editor, "a");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
}

#[test]
fn an_undo_inside_a_command_seals_the_edits_before_it() {
    let (mut editor, id) = editor_with("abc");
    plain_edit(&mut editor, id, 0..0, "1");
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit(1..1, "2")]).unwrap();
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "1abc", "undoes this command's edit");
    assert_eq!(cursors(&editor.windows[0]), [1]);
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc");
    assert_eq!(cursors(&editor.windows[0]), [0]);
    editor.apply_edits(id, &[edit(1..1, "3")]).unwrap();
    editor.end_undo_group();
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc");
    assert_eq!(cursors(&editor.windows[0]), [0], "as the undo left them");
}

#[test]
fn a_failed_redo_inside_a_command_does_not_split_its_group() {
    let (mut editor, id) = editor_with("abc");
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit(0..0, "1")]).unwrap();
    assert_eq!(editor.redo(), Err(EditError::NoRedo));
    editor.apply_edits(id, &[edit(0..0, "2")]).unwrap();
    editor.end_undo_group();
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
}

#[test]
fn edits_move_every_field_of_a_selection_that_holds_an_offset() {
    // This builds the selection field by field, so a field added to `Selection`
    // has to be thought about here.
    let selection = Selection {
        cursor: 2,
        mark: Some(4),
        goal: Some(crate::selection::Goal::LineEnd),
    };
    let (mut editor, id) = editor_with("abcdef");
    editor.windows[0].selections = SelectionSet::from_parts(vec![selection], 0).unwrap();
    editor.apply_edits(id, &[edit(0..0, "x")]).unwrap();
    let moved = Selection {
        mark: Some(5),
        ..Selection::at(3)
    };
    assert_eq!(*editor.window().selections.primary(), moved);
}

#[test]
fn a_buffer_killed_in_a_command_is_no_trouble() {
    let (mut editor, _) = editor_with("abc");
    let other = editor.add_buffer(Buffer::from_text("o", "x"));
    editor.begin_undo_group();
    editor.apply_edits(other, &[edit(0..0, "y")]).unwrap();
    editor.kill_buffer(other);
    editor.end_undo_group();
    assert!(editor.buffer(other).is_none());
}

#[test]
fn an_edit_of_another_buffer_in_a_command_is_its_own_group_there() {
    let (mut editor, id) = editor_with("abc");
    let other = editor.add_buffer(Buffer::from_text("o", "x"));
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit(0..0, "1")]).unwrap();
    editor.apply_edits(other, &[edit(0..0, "2")]).unwrap();
    editor.apply_edits(id, &[edit(0..0, "3")]).unwrap();
    editor.end_undo_group();
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
    assert_eq!(editor.buffer(other).unwrap().undo_lens(), (1, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc");
    assert_eq!(text(&editor, other), "2x", "undo works on the shown buffer");
}

#[test]
fn a_group_in_a_buffer_not_shown_has_no_cursors_from_the_selected_window() {
    let (mut editor, id) = editor_with("0123456789");
    let other = editor.add_buffer(Buffer::from_text("o", "abc"));
    let window = second_window(&mut editor, other, &[1]);
    editor.windows[0].selections = SelectionSet::new(2);
    assert_eq!(editor.window().buffer(), id);
    plain_edit(&mut editor, other, 0..0, "x");
    editor.selected = window;
    assert_eq!(cursors(&editor.windows[window]), [2]);
    editor.undo().unwrap();
    assert_eq!(text(&editor, other), "abc");
    assert_eq!(cursors(&editor.windows[window]), [1], "follows the text");
}

#[test]
fn the_byte_bound_leaves_out_the_newest_group() {
    let (mut editor, id) = editor_with(&"x".repeat(5000));
    editor.buffer_mut(id).unwrap().set_undo_limit(2000);
    plain_edit(&mut editor, id, 0..100, "");
    // Over the bound alone, it stays, and so does the older group.
    plain_edit(&mut editor, id, 0..3000, "");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (2, 0));
    // No longer the newest, it goes, and the older group before it.
    plain_edit(&mut editor, id, 0..1, "");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id).len(), 1900);
    assert_eq!(editor.undo(), Err(EditError::NoUndo));
}

#[test]
fn undoing_a_huge_paste_keeps_older_groups_and_can_be_redone() {
    let (mut editor, id) = editor_with("abc");
    editor.buffer_mut(id).unwrap().set_undo_limit(2000);
    plain_edit(&mut editor, id, 0..0, "1");
    plain_edit(&mut editor, id, 0..0, &"p".repeat(5000));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "1abc");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (1, 1));
    editor.redo().unwrap();
    assert_eq!(text(&editor, id).len(), 5004);
    editor.undo().unwrap();
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc");
    assert!(!editor.buffer(id).unwrap().is_modified());
}

#[test]
fn history_dropped_past_the_saved_state_stays_modified_until_saved() {
    let (mut editor, id) = editor_with(&"x".repeat(5000));
    editor.buffer_mut(id).unwrap().set_undo_limit(2000);
    plain_edit(&mut editor, id, 0..100, "");
    let saved = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(saved);
    plain_edit(&mut editor, id, 0..100, "");
    plain_edit(&mut editor, id, 0..3000, "");
    plain_edit(&mut editor, id, 0..1, "");
    // The group that returned to the saved state is gone: only the last one
    // is left, which returns to a state past saved.
    editor.undo().unwrap();
    assert_eq!(editor.undo(), Err(EditError::NoUndo));
    assert!(editor.buffer(id).unwrap().is_modified());
    editor.redo().unwrap();
    assert!(editor.buffer(id).unwrap().is_modified());
    let now = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(now);
    assert!(!editor.buffer(id).unwrap().is_modified());
}

#[test]
fn undoing_a_huge_kill_brings_back_the_text() {
    let line = "日本語 tab\there e\u{301}\r\n";
    let start = line.repeat(60);
    let (mut editor, id) = editor_with(&start);
    let end = start.len() - line.len();
    plain_edit(&mut editor, id, line.len()..end, "");
    assert_eq!(text(&editor, id), line.repeat(2));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), start);
    let buffer = editor.buffer(id).unwrap();
    buffer.text().check_invariants();
    let tab_width = TabWidth::default();
    crate::display_map::tests::check_against(&buffer.display(), &start, tab_width);
}

#[test]
fn a_step_kept_apart_joins_neither_side() {
    let (mut editor, id) = editor_with("");
    self_insert(&mut editor, "a");
    editor.begin_undo_group();
    editor.keep_undo_step_apart();
    editor.allow_undo_join();
    let end = editor.windows[0].selections.primary().cursor;
    editor.apply_edits(id, &[edit(end..end, "あ")]).unwrap();
    editor.end_undo_group();
    self_insert(&mut editor, "b");
    assert_eq!(text(&editor, id), "aあb");
    assert_eq!(editor.buffer(id).unwrap().undo_lens(), (3, 0));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "aあ");
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "a");
}

#[test]
fn an_edit_hides_the_regions_of_every_window_showing_the_buffer() {
    let (mut editor, id) = editor_with("abcdef");
    let other = second_window(&mut editor, id, &[5]);
    for window in &mut editor.windows {
        window.selections.push_marks();
        window.selections.set_region(Region::Active);
    }
    plain_edit(&mut editor, id, 0..0, "x");
    for window in &editor.windows {
        assert_eq!(window.selections.region(), Region::Inactive);
    }
    // The marks stay, and follow.
    assert_eq!(editor.windows[other].selections.primary().mark, Some(6));
    assert_eq!(editor.window().selections.primary().mark, Some(0));
}

#[test]
fn undo_puts_back_the_marks_hides_the_region_and_leaves_the_ring() {
    let (mut editor, id) = editor_with("abcdef");
    let set = |cursor, mark| {
        let selection = Selection {
            mark: Some(mark),
            ..Selection::at(cursor)
        };
        SelectionSet::from_parts(vec![selection], 0).unwrap()
    };
    editor.windows[0].selections = set(2, 4);
    plain_edit(&mut editor, id, 0..0, "x");
    // A mark pushed after the edit stays in the ring through the undo.
    editor.windows[0].selections = set(1, 6);
    editor.windows[0].selections.push_marks();
    editor.windows[0].selections.set_region(Region::Active);
    editor.undo().unwrap();
    let selections = &editor.window().selections;
    assert_eq!(*selections.primary(), *set(2, 4).primary());
    assert_eq!(selections.region(), Region::Inactive);
    assert_eq!(
        selections.mark_ring().iter().collect::<Vec<_>>(),
        [5],
        "6 less the x"
    );
}
