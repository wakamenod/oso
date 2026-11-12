//! Tests of visiting a file at a place (ADR 0114): at once in a buffer shown, or once the file
//! has loaded, with the place before on the mark ring.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;

use crate::buffer::Buffer;
use crate::editor::Editor;

/// An editor showing `/w/oso/a.rs`, loaded with `text`.
fn showing(text: &[u8]) -> Editor {
    let mut buffer = Buffer::visiting(PathBuf::from("/w/oso/a.rs"));
    buffer.apply_load(crate::load::decode_all(text, Default::default()));
    Editor::with_buffer(buffer)
}

#[test]
fn a_visit_at_a_place_goes_there_with_the_place_before_on_the_mark_ring() {
    let mut editor = showing("one\ntwo ünïcode needle\nthree\n".as_bytes());
    editor.windows[0].selections.move_primary(2);
    editor.request_visit_at(PathBuf::from("/w/oso/a.rs"), 1 << 20, 2, 12);
    assert!(editor.take_visits().is_empty(), "it is open already");
    let selections = &editor.windows[0].selections;
    let line = "two ünïcode needle";
    let column_12 = line.char_indices().nth(12).unwrap().0;
    assert_eq!(selections.primary().cursor, 4 + column_12);
    assert_eq!(selections.primary().mark, Some(2), "the place before");
    // A line past the end goes to the last, and a column past the line to its end.
    editor.request_visit_at(PathBuf::from("/w/oso/a.rs"), 1 << 20, 99, 99);
    assert_eq!(editor.windows[0].selections.primary().cursor, 31);
}

#[test]
fn a_visit_at_a_place_waits_for_its_file_to_show() {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
    editor.request_visit_at(PathBuf::from("/w/oso/a.rs"), 1 << 20, 2, 1);
    assert_eq!(editor.take_visits().len(), 1);
    let mut buffer = Buffer::visiting(PathBuf::from("/w/oso/a.rs"));
    buffer.apply_load(crate::load::decode_all(b"ab\ncd\n", Default::default()));
    let id = editor.add_buffer(buffer);
    editor.show(id);
    editor.loaded(id);
    assert_eq!(editor.windows[0].selections.primary().cursor, 4);
    // A visit with no place after it forgets the place.
    editor.request_visit_at(PathBuf::from("/w/oso/b.rs"), 1 << 20, 2, 1);
    editor.request_visit(PathBuf::from("/w/oso/c.rs"), 1 << 20);
    assert!(editor.visit_place.is_none());
}
