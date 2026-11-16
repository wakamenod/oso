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
    editor.jump_to_place(b, 4);
    assert_eq!(cursor(&editor), (b, 4));
    assert_eq!(editor.window().selections.len(), 1);
    editor.jump_to_place(b, 0);
    assert_eq!(cursor(&editor), (b, 0));
    assert_eq!(editor.jump_back(), Ok(()));
    assert_eq!(cursor(&editor), (b, 4));
    assert_eq!(editor.jump_back(), Ok(()));
    assert_eq!(cursor(&editor), (a, 4));
    assert_eq!(editor.jump_back(), Err("No earlier place"));
    assert_eq!(editor.jump_forward(), Ok(()));
    assert_eq!(cursor(&editor), (b, 4));
    assert_eq!(editor.jump_forward(), Ok(()));
    assert_eq!(cursor(&editor), (b, 0));
    assert_eq!(editor.jump_forward(), Err("No later place"));
    // The place left is on the mark ring of the buffer it was in.
    assert_eq!(editor.jump_back(), Ok(()));
    assert_eq!(editor.jump_back(), Ok(()));
    assert_eq!(editor.window().selections.primary().mark, Some(4));
}

#[test]
fn a_place_follows_edits_and_goes_with_its_buffer() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "abc def\n"));
    let a = editor.window().buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "xyz\n"));
    editor.current_mut().0.selections = crate::selection::SelectionSet::new(4);
    editor.jump_to_place(b, 1);
    editor
        .apply_edits(
            a,
            &[Replacement {
                range: 0..0,
                text: "// ",
            }],
        )
        .unwrap();
    assert_eq!(editor.jump_back(), Ok(()));
    assert_eq!(cursor(&editor), (a, 7), "moved by what went in before it");
    editor.jump_to_place(b, 2);
    editor.kill_buffer(b);
    assert_eq!(editor.window().jumps().len(), (1, 0));
    assert_eq!(editor.jump_back(), Ok(()));
    assert_eq!(cursor(&editor).0, a);
}

#[test]
fn a_column_is_read_as_its_source_counts_it() {
    // `é` is two bytes, one UTF-16 unit; `😀` four bytes, two units.
    let text = Rope::from("x\né😀z\n");
    let line = text.line_start(1);
    assert_eq!(column_offset(&text, 1, Column::Chars(2)), line + 6);
    assert_eq!(column_offset(&text, 1, Column::Utf8(6)), line + 6);
    assert_eq!(
        column_offset(&text, 1, Column::Utf8(3)),
        line + 2,
        "back to a character's start"
    );
    assert_eq!(column_offset(&text, 1, Column::Utf16(3)), line + 6);
    assert_eq!(
        column_offset(&text, 1, Column::Utf16(99)),
        line + 7,
        "as far as the line goes"
    );
    assert_eq!(
        column_offset(&text, 9, Column::Chars(0)),
        text.len(),
        "the last line"
    );
}
