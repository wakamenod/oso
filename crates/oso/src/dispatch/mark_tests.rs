//! Scenario tests of the mark, the region and the mark ring, with their keys,
//! through the [`Dispatcher`] (ADR 0037, ADR 0071).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::editor::Editor;
use oso_core::selection::MarkActive;

use super::motion_tests::{NOTHING, at, cursors, send, text};
use crate::commands::kept_messages;

/// The primary cursor's mark.
fn mark(editor: &Editor) -> Option<usize> {
    editor.window().selections.primary().mark
}

fn mark_active(editor: &Editor) -> MarkActive {
    editor.window().selections.mark_active()
}

fn ring(editor: &Editor) -> Vec<usize> {
    editor.window().selections.mark_ring().iter().collect()
}

fn last_message(editor: &Editor) -> Option<String> {
    kept_messages(editor).pop()
}

#[test]
fn c_spc_sets_the_mark_and_shows_the_region_and_again_hides_it() {
    let (mut d, mut editor) = at("one two\n", &[2]);
    assert!(send(&mut d, &mut editor, "C-SPC").redraw);
    assert_eq!(
        (mark(&editor), mark_active(&editor)),
        (Some(2), MarkActive::Active)
    );
    assert_eq!(last_message(&editor).as_deref(), Some("Mark set"));
    send(&mut d, &mut editor, "C-f C-f");
    assert_eq!(mark_active(&editor), MarkActive::Active, "motion keeps it");
    assert_eq!(editor.window().selections.primary().range(), 2..4);

    send(&mut d, &mut editor, "C-SPC");
    assert_eq!(ring(&editor), [2], "the old mark");
    assert!(send(&mut d, &mut editor, "C-SPC").redraw);
    assert_eq!(
        (mark(&editor), mark_active(&editor)),
        (Some(4), MarkActive::Inactive)
    );
    assert_eq!(last_message(&editor).as_deref(), Some("Mark deactivated"));
    send(&mut d, &mut editor, "C-@");
    assert_eq!(
        mark_active(&editor),
        MarkActive::Active,
        "a third sets it again"
    );
}

#[test]
fn an_edit_c_g_and_undo_hide_the_region_and_keep_the_mark() {
    let (mut d, mut editor) = at("abc\n", &[1]);
    send(&mut d, &mut editor, "C-SPC C-f x");
    assert_eq!(
        text(&editor),
        "axc\n",
        "typing replaces the region (ADR 0074)"
    );
    assert_eq!(
        (mark(&editor), mark_active(&editor)),
        (Some(1), MarkActive::Inactive)
    );
    send(&mut d, &mut editor, "C-x C-x");
    assert_eq!(mark_active(&editor), MarkActive::Active);
    send(&mut d, &mut editor, "C-z");
    assert_eq!(mark_active(&editor), MarkActive::Inactive);
    send(&mut d, &mut editor, "C-x C-x");
    assert!(send(&mut d, &mut editor, "C-g").redraw);
    assert_eq!(mark_active(&editor), MarkActive::Inactive);
    assert_eq!(
        send(&mut d, &mut editor, "C-g"),
        NOTHING,
        "nothing more to hide"
    );
}

#[test]
fn c_x_c_x_swaps_every_cursor_with_its_mark() {
    let (mut d, mut editor) = at("ab\ncd\n", &[0, 3]);
    let result = d
        .run_command(&mut editor, "exchange-point-and-mark", None)
        .1;
    assert_eq!(
        result,
        Err("exchange-point-and-mark: No mark set in this buffer".into())
    );
    send(&mut d, &mut editor, "C-SPC C-SPC C-f");
    assert_eq!(mark_active(&editor), MarkActive::Inactive);
    send(&mut d, &mut editor, "C-x C-x");
    assert_eq!(cursors(&editor), [0, 3]);
    assert_eq!(mark_active(&editor), MarkActive::Active, "shown");
    send(&mut d, &mut editor, "C-u C-x C-x");
    assert_eq!(cursors(&editor), [1, 4]);
    assert_eq!(
        mark_active(&editor),
        MarkActive::Inactive,
        "with C-u, hidden"
    );
}

#[test]
fn c_u_c_spc_and_c_comma_go_back_through_the_ring() {
    let (mut d, mut editor) = at("0123456789\n", &[1]);
    send(
        &mut d,
        &mut editor,
        "C-SPC C-SPC C-f C-f C-SPC C-SPC C-f C-f C-SPC C-SPC",
    );
    assert_eq!((mark(&editor), ring(&editor)), (Some(5), vec![3, 1]));
    send(&mut d, &mut editor, "C-e");
    send(&mut d, &mut editor, "C-u C-SPC");
    assert_eq!((cursors(&editor), mark(&editor)), (vec![5], Some(3)));
    assert_eq!(mark_active(&editor), MarkActive::Inactive);
    send(&mut d, &mut editor, "C-,");
    assert_eq!(cursors(&editor), [3]);
    send(&mut d, &mut editor, "C-, C-,");
    assert_eq!(cursors(&editor), [5], "round the ring");
    // Right after C-u C-SPC, C-SPC sets the mark (`set-mark-command-repeat-pop` nil).
    send(&mut d, &mut editor, "C-u C-SPC C-SPC");
    assert_eq!(mark_active(&editor), MarkActive::Active);

    let (mut d, mut editor) = at("ab\n", &[1]);
    let result = d.run_command(&mut editor, "pop-to-mark-command", None).1;
    assert_eq!(
        result,
        Err("pop-to-mark-command: No mark set in this buffer".into())
    );
    send(&mut d, &mut editor, "C-SPC C-SPC C-,");
    assert_eq!(last_message(&editor).as_deref(), Some("Mark popped"));
}

#[test]
fn jumping_to_an_end_sets_the_mark_to_come_back_to() {
    let (mut d, mut editor) = at("one\ntwo\n", &[2]);
    send(&mut d, &mut editor, "M->");
    assert_eq!(
        (mark(&editor), mark_active(&editor)),
        (Some(2), MarkActive::Inactive)
    );
    assert_eq!(last_message(&editor).as_deref(), Some("Mark set"));
    send(&mut d, &mut editor, "C-u C-SPC");
    assert_eq!(cursors(&editor), [2]);
    // Not with C-u, which goes 4/10 of the way, nor while the region shows, which
    // the jump extends.
    send(&mut d, &mut editor, "C-u M-<");
    assert_eq!(cursors(&editor), [4]);
    assert_eq!(mark(&editor), Some(2), "unchanged");
    send(&mut d, &mut editor, "C-SPC M->");
    assert_eq!(
        (mark(&editor), mark_active(&editor)),
        (Some(4), MarkActive::Active)
    );
}

#[test]
fn c_x_h_marks_the_whole_buffer_with_one_cursor() {
    let (mut d, mut editor) = at("ab\ncd\n", &[1, 4]);
    send(&mut d, &mut editor, "C-SPC C-SPC");
    send(&mut d, &mut editor, "C-x h");
    let selections = &editor.window().selections;
    assert_eq!((cursors(&editor), mark(&editor)), (vec![0], Some(6)));
    assert_eq!(selections.mark_active(), MarkActive::Active);
    assert_eq!(
        ring(&editor),
        [1, 1],
        "the old mark, then where the cursor was"
    );
}

#[test]
fn shift_selects_from_every_cursor_until_a_motion_without_it() {
    let (mut d, mut editor) = at("one two\nthree four\n", &[0, 8]);
    send(&mut d, &mut editor, "S-<right> M-F");
    assert_eq!(mark_active(&editor), MarkActive::Shifted);
    let ranges: Vec<_> = editor
        .window()
        .selections
        .iter()
        .map(|s| s.range())
        .collect();
    assert_eq!(ranges, [0..3, 8..13]);
    send(&mut d, &mut editor, "C-V");
    assert_eq!(
        mark_active(&editor),
        MarkActive::Shifted,
        "scrolling with Shift too"
    );
    assert!(send(&mut d, &mut editor, "<right>").redraw);
    assert_eq!(mark_active(&editor), MarkActive::Inactive);
    // A region set with C-SPC stays through motion with or without Shift.
    send(&mut d, &mut editor, "C-SPC S-<left> C-b");
    assert_eq!(mark_active(&editor), MarkActive::Active);
}

#[test]
fn overlapping_regions_of_several_cursors_merge() {
    let (mut d, mut editor) = at("abcdefgh\n", &[1, 4]);
    send(&mut d, &mut editor, "C-SPC C-f C-f C-f C-f");
    let selections = &editor.window().selections;
    assert_eq!(selections.len(), 1);
    assert_eq!(selections.primary().range(), 1..8);
}

#[test]
fn a_region_in_messages_stays_through_the_messages_logged_there() {
    let (mut d, mut editor) = at("ab\n", &[0]);
    send(&mut d, &mut editor, "C-h e M-< C-SPC C-f");
    assert_eq!(editor.current().1.name(), "*Messages*");
    // "Mark set" itself went to the end of *Messages*.
    assert_eq!(mark_active(&editor), MarkActive::Active);
    editor.message("another");
    assert_eq!(mark_active(&editor), MarkActive::Active);
}

#[test]
fn c_spc_at_the_end_of_messages_shows_no_region_over_what_it_logs() {
    let (mut d, mut editor) = at("ab\n", &[0]);
    send(&mut d, &mut editor, "C-h e C-SPC");
    let selections = &editor.window().selections;
    assert_eq!(selections.mark_active(), MarkActive::Active);
    assert!(
        selections.primary().range().is_empty(),
        "the mark went along"
    );
    send(&mut d, &mut editor, "M->");
    assert!(editor.window().selections.primary().range().is_empty());
}
