//! Tests of what the host relies on while an undo or a redo goes on over turns (ADR 0119):
//! what a crossing lets go waits to be freed, its progress comes before a message, a reload
//! waits for it, and a large batch is recorded in parts that cross as the whole.
//!
//! Must not depend on: `command` or `keymap`; these drive the editor directly, as the host
//! would.

use super::super::edit_tests::{edit, editor_with, text};
use super::tests::{SPLICE_WORK_IN_TESTS, echo, finish};
use super::*;
use crate::buffer::Buffer;

/// An editor whose one step undoes in three slices of one splice.
fn three_splices() -> (Editor, BufferId) {
    let (mut editor, id) = editor_with("one two three");
    editor.begin_undo_group();
    let edits = [edit(0..3, "1"), edit(4..7, "2"), edit(8..13, "3")];
    editor.apply_edits(id, &edits).expect("an edit");
    editor.end_undo_group();
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    (editor, id)
}

#[test]
fn what_a_crossing_lets_go_waits_for_the_host_to_free_it() {
    // A step that commits.
    let (mut editor, _) = three_splices();
    assert!(editor.take_crossing_freed().is_none());
    editor.undo().expect("an undo");
    assert!(editor.take_crossing_freed().is_some());
    assert!(editor.take_crossing_freed().is_none(), "taken once");
    // One cancelled.
    let (mut editor, _) = three_splices();
    editor.begin_crossing(1).expect("an undo");
    assert!(editor.cancel_crossing());
    assert!(editor.take_crossing_freed().is_some());
    // One that another begun replaced.
    let (mut editor, _) = three_splices();
    editor.begin_crossing(1).expect("an undo");
    editor.begin_crossing(1).expect("an undo");
    assert!(editor.take_crossing_freed().is_some());
    // And one abandoned.
    let (mut editor, id) = three_splices();
    editor.begin_crossing(1).expect("an undo");
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit(0..0, "!")]).expect("an edit");
    editor.end_undo_group();
    assert_eq!(editor.go_on_crossing(), Some(Err(EditError::Abandoned)));
    assert!(editor.take_crossing_freed().is_some());
}

#[test]
fn the_progress_comes_before_a_message_until_the_crossing_ends() {
    let (mut editor, id) = editor_with(&"word ".repeat(8));
    editor.begin_undo_group();
    let edits: Vec<_> = (0..8).map(|word| edit(word * 5..word * 5, "a")).collect();
    editor.apply_edits(id, &edits).expect("an edit");
    editor.end_undo_group();
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    editor.begin_crossing(1).expect("an undo");
    editor.message("Count too large");
    assert_eq!(echo(&editor), "Count too large", "before the second slice");
    editor.go_on_crossing();
    assert_eq!(echo(&editor), "Undoing… 25%");
    finish(&mut editor, Ok(())).expect("the undo");
    assert_eq!(echo(&editor), "Count too large");
}

#[test]
fn a_reload_waits_for_a_crossing() {
    let (mut editor, _) = three_splices();
    editor.begin_crossing(1).expect("an undo");
    assert!(editor.reload_waits());
    finish(&mut editor, Ok(())).expect("the undo");
    assert!(!editor.reload_waits());
}

#[test]
fn a_batch_larger_than_a_part_is_recorded_in_parts_and_crosses_as_the_whole() {
    let parts = 2;
    let count = crate::undo::BATCH_SPLICES * parts + 1;
    let before = "ab".repeat(count);
    for meeting in [false, true] {
        let (mut editor, id) = editor_with(&before);
        editor.begin_undo_group();
        // Each deletion meets the next, or a byte lies between them.
        let edits: Vec<_> = if meeting {
            (0..count).map(|at| edit(at * 2..at * 2 + 2, "c")).collect()
        } else {
            (0..count).map(|at| edit(at * 2..at * 2 + 1, "")).collect()
        };
        editor.apply_edits(id, &edits).expect("an edit");
        editor.end_undo_group();
        let after = text(&editor, id);
        let seen = editor
            .buffer(id)
            .map(Buffer::history_seen)
            .unwrap_or_default();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].2.len(), parts + 1, "parts of the batch");
        for work in [usize::MAX, 1000] {
            editor.set_crossing_slices(work, 4);
            let begun = editor.begin_crossing(1);
            finish(&mut editor, begun).expect("an undo");
            assert_eq!(text(&editor, id), before);
            let begun = editor.begin_crossing(-1);
            finish(&mut editor, begun).expect("a redo");
            assert_eq!(text(&editor, id), after);
        }
    }
}
