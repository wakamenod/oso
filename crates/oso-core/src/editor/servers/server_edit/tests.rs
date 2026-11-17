//! Tests of a server's edit applied (ADR 0138, step 13): whole, one undo step a buffer, the
//! insertions at one place in the server's order, or not at all for a buffer stale, gone, named
//! twice, read-only, loading or that may hold secrets, or for edits that overlap.
//!
//! Must not depend on: threads or the file system.

use super::*;
use crate::buffer::Buffer;
use crate::frame::FrameSize;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 120,
    });
    editor
}

fn part(editor: &Editor, buffer: BufferId, edits: &[(Range<usize>, &str)]) -> BufferEdit {
    BufferEdit {
        buffer,
        revision: editor.buffer(buffer).unwrap().revision(),
        edits: (edits.iter())
            .map(|(range, text)| (range.clone(), (*text).to_owned()))
            .collect(),
    }
}

fn text(editor: &Editor, id: BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

#[test]
fn an_edit_applies_to_each_buffer_in_one_undo_step() {
    let mut editor = editor("let x = 1;\n");
    let a = editor.tiled().0.buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "fn b() {}\n"));
    let edit = [
        // Two insertions at one place go in the server's order.
        part(
            &editor,
            a,
            &[(0..0, "use std;\n"), (4..5, "y"), (0..0, "use core;\n")],
        ),
        part(&editor, b, &[(3..4, "c")]),
    ];
    let applied = editor.apply_server_edit(edit.to_vec()).unwrap();
    assert_eq!(
        applied,
        EditApplied {
            edits: 3,
            buffers: 2
        }
    );
    assert_eq!(text(&editor, a), "use std;\nuse core;\nlet y = 1;\n");
    assert_eq!(text(&editor, b), "fn c() {}\n");
    editor.undo().unwrap();
    assert_eq!(text(&editor, a), "let x = 1;\n", "one step");
}

#[test]
fn an_edit_that_is_stale_overlaps_or_meets_a_secret_changes_nothing() {
    let mut editor = editor("let x = 1;\n");
    let a = editor.tiled().0.buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "fn b() {}\n"));
    let fine = part(&editor, a, &[(4..5, "y")]);
    let mut stale = part(&editor, b, &[(3..4, "c")]);
    stale.revision += 1;
    assert_eq!(
        editor.apply_server_edit(vec![fine.clone(), stale]),
        Err("A buffer it edits changed since the server answered")
    );
    let overlapping = part(&editor, b, &[(0..5, "x"), (3..6, "y")]);
    assert_eq!(
        editor.apply_server_edit(vec![fine.clone(), overlapping]),
        Err("Its edits overlap")
    );
    let gone = BufferEdit {
        buffer: BufferId(999),
        ..fine.clone()
    };
    assert!(editor.apply_server_edit(vec![gone]).is_err());
    assert_eq!(text(&editor, a), "let x = 1;\n", "nothing applied");
    assert_eq!(text(&editor, b), "fn b() {}\n");
    let path = std::path::PathBuf::from("/w/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(std::slice::from_ref(&path)));
    let secret = editor.add_buffer(secret);
    let edit = part(&editor, secret, &[(0..3, "K")]);
    assert_eq!(
        editor.apply_server_edit(vec![edit]),
        Err("No server edits a buffer that may hold secrets")
    );
}

#[test]
fn an_edit_of_a_buffer_named_twice_read_only_or_loading_changes_nothing() {
    let mut editor = editor("let x = 1;\n");
    let a = editor.tiled().0.buffer();
    let twice = vec![
        part(&editor, a, &[(0..1, "L")]),
        part(&editor, a, &[(4..5, "y")]),
    ];
    assert_eq!(
        editor.apply_server_edit(twice),
        Err("It edits a buffer twice")
    );
    let read_only = editor.add_buffer(Buffer::internal("*internal*"));
    let edit = vec![
        part(&editor, a, &[(0..1, "L")]),
        part(&editor, read_only, &[(0..0, "y")]),
    ];
    assert_eq!(
        editor.apply_server_edit(edit),
        Err("A buffer it edits is read-only")
    );
    let loading = editor.add_buffer(Buffer::visiting(std::path::PathBuf::from("/w/l.rs")));
    let edit = vec![part(&editor, loading, &[(0..0, "y")])];
    assert!(editor.apply_server_edit(edit).is_err(), "still loading");
    assert_eq!(text(&editor, a), "let x = 1;\n", "nothing applied");
}
