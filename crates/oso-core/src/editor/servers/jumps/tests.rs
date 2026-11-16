//! Tests of the jumps a server's answers lead to: one cursor at the target, the history walked
//! back and forward, places that follow edits and go with their buffer, and columns as their
//! source counts them.
//!
//! Must not depend on: the host or its servers; these drive the editor alone.

use super::*;
use crate::buffer::{Buffer, Replacement};

fn cursor(editor: &Editor) -> (BufferId, usize) {
    let window = editor.window();
    (window.buffer(), window.selections.primary().cursor)
}

#[test]
fn a_jump_goes_there_with_one_cursor_and_back_and_forward_walk_the_history() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "fn a() {}\nfn b() {}\n"));
    let a = editor.window().buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "use a;\n"));
    editor.current_mut().0.selections = crate::selection::SelectionSet::new(4);
    editor.xref_to_place(b, 4);
    assert_eq!(cursor(&editor), (b, 4));
    assert_eq!(editor.window().selections.len(), 1);
    editor.xref_to_place(b, 0);
    assert_eq!(cursor(&editor), (b, 0));
    assert_eq!(editor.xref_back(), Ok(()));
    assert_eq!(cursor(&editor), (b, 4));
    assert_eq!(editor.xref_back(), Ok(()));
    assert_eq!(cursor(&editor), (a, 4));
    assert_eq!(editor.xref_back(), Err("No earlier place"));
    assert_eq!(editor.xref_forward(), Ok(()));
    assert_eq!(cursor(&editor), (b, 4));
    assert_eq!(editor.xref_forward(), Ok(()));
    assert_eq!(cursor(&editor), (b, 0));
    assert_eq!(editor.xref_forward(), Err("No later place"));
    // The place left is on the mark ring of the buffer it was in.
    assert_eq!(editor.xref_back(), Ok(()));
    assert_eq!(editor.xref_back(), Ok(()));
    assert_eq!(editor.window().selections.primary().mark, Some(4));
}

#[test]
fn a_place_follows_edits_and_goes_with_its_buffer() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "abc def\n"));
    let a = editor.window().buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "xyz\n"));
    editor.current_mut().0.selections = crate::selection::SelectionSet::new(4);
    editor.xref_to_place(b, 1);
    editor
        .apply_edits(
            a,
            &[Replacement {
                range: 0..0,
                text: "// ",
            }],
        )
        .unwrap();
    assert_eq!(editor.xref_back(), Ok(()));
    assert_eq!(cursor(&editor), (a, 7), "moved by what went in before it");
    editor.xref_to_place(b, 2);
    editor.kill_buffer(b);
    assert_eq!(editor.window().jumps().counts(), (1, 0));
    assert_eq!(editor.xref_back(), Ok(()));
    assert_eq!(cursor(&editor).0, a);
}

#[test]
fn an_undo_moves_a_place_back_as_the_edit_moved_it() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "abc def\n"));
    let a = editor.window().buffer();
    editor.buffer_mut(a).unwrap().set_followed(true);
    let b = editor.add_buffer(Buffer::from_text("b.rs", "xyz\n"));
    editor.current_mut().0.selections = crate::selection::SelectionSet::new(4);
    editor.xref_to_place(b, 1);
    editor.show(a);
    crate::editor::edit_tests::plain_edit(&mut editor, a, 0..0, "// ");
    editor.undo().unwrap();
    editor.show(b);
    assert_eq!(editor.xref_back(), Ok(()));
    assert_eq!(cursor(&editor), (a, 4), "the undo took the place back");
}

#[test]
fn a_region_keeps_its_mark_and_a_jump_to_where_the_cursor_is_does_nothing() {
    use crate::selection::{MarkActive, SelectionSet};
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "abcdefgh\n"));
    let a = editor.window().buffer();
    let mut set = SelectionSet::new(5);
    set.push_primary_mark(1);
    set.set_mark_active(MarkActive::Active);
    editor.current_mut().0.selections = set;
    editor.xref_to_place(a, 7);
    let primary = editor.window().selections.primary();
    assert_eq!(
        (primary.cursor, primary.mark),
        (7, Some(1)),
        "the region's start stays"
    );
    editor.xref_to_place(a, 7);
    assert_eq!(
        editor.window().jumps().counts(),
        (1, 0),
        "no place for a jump that stays"
    );
}

#[test]
fn the_agent_goes_back_to_no_buffer_it_may_not_be_shown() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "abc\n"));
    let a = editor.window().buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "xyz\n"));
    editor.xref_to_place(b, 1);
    assert!(editor.buffer_mut(a).unwrap().treat_as_secret());
    assert_eq!(
        editor.for_agent(Editor::xref_back),
        Err("That place's buffer is not for the agent")
    );
    assert_eq!(
        editor.window().jumps().counts(),
        (1, 0),
        "the history is as it was"
    );
    assert_eq!(editor.xref_back(), Ok(()), "the person may");
    assert_eq!(cursor(&editor).0, a);
}
