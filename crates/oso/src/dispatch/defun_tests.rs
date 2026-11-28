//! Scenario tests of motion by defuns, `mark-defun` and `reposition-window`, with their
//! keys, through the [`Dispatcher`] (ADR 0190, step 3).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::selection::MarkActive;
use oso_core::syntax::{Highlight, Highlights};

use super::Dispatcher;
use super::motion_tests::{at, cursors, place_at, send};
use crate::commands::kept_messages;

const RUST: &str = "use a;\n\
\n\
// S.\n\
struct S {\n\
\x20   a: u32,\n\
}\n\
\n\
impl S {\n\
\x20   fn one() {\n\
\x20       x();\n\
\x20   }\n\
\n\
\x20   fn two() {}\n\
}\n";

fn offset(find: &str) -> usize {
    RUST.find(find).expect("in RUST")
}

/// A dispatcher and an editor showing `RUST` in Rust mode, with a cursor at each of
/// `cursors`.
fn rust_at(cursors: &[usize]) -> (Dispatcher, Editor) {
    let (dispatcher, mut editor) = at(RUST, cursors);
    let id = editor.window().buffer();
    assert!(editor.buffer_mut(id).unwrap().set_mode(Mode::Rust));
    (dispatcher, editor)
}

fn mark(editor: &Editor) -> Option<usize> {
    editor.window().selections.primary().mark
}

fn mark_active(editor: &Editor) -> MarkActive {
    editor.window().selections.mark_active()
}

fn messages(editor: &Editor) -> Vec<String> {
    kept_messages(editor)
}

fn ranges(editor: &Editor) -> Vec<std::ops::Range<usize>> {
    editor
        .window()
        .selections
        .iter()
        .map(|selection| selection.range())
        .collect()
}

/// The region of the one cursor left.
fn region(editor: &Editor) -> std::ops::Range<usize> {
    let selections = &editor.window().selections;
    assert_eq!(selections.len(), 1, "one cursor");
    selections.primary().range()
}

#[test]
fn c_m_a_and_c_m_e_move_every_cursor_by_defuns() {
    let (mut d, mut editor) = rust_at(&[offset("a: u32"), offset("x();")]);
    assert!(send(&mut d, &mut editor, "C-M-a").redraw);
    assert_eq!(cursors(&editor), [offset("struct S"), offset("fn one")]);
    send(&mut d, &mut editor, "C-M-e");
    assert_eq!(cursors(&editor), [offset("\nimpl"), offset("\n    fn two")]);
    // The other keys, a negative count, and one past the first.
    send(&mut d, &mut editor, "C-M-<home>");
    assert_eq!(cursors(&editor), [offset("struct S"), offset("fn one")]);
    assert!(send(&mut d, &mut editor, "M-- C-M-<end>").redraw);
    assert_eq!(cursors(&editor), [0, offset("\nimpl")]);
    send(&mut d, &mut editor, "C-M-a");
    assert_eq!(cursors(&editor), [0, offset("struct S")]);
    send(&mut d, &mut editor, "C-u 2 C-M-a");
    assert_eq!(cursors(&editor), [0], "they met");
    // Cursors that meet merge.
    place_at(&mut editor, &[0, offset("// S.")]);
    send(&mut d, &mut editor, "C-M-e");
    assert_eq!(cursors(&editor), [offset("\nimpl")]);
}

#[test]
fn past_the_last_defun_goes_to_the_edge_quietly() {
    let (mut d, mut editor) = rust_at(&[offset("struct S")]);
    assert!(send(&mut d, &mut editor, "C-M-a").redraw);
    assert_eq!(cursors(&editor), [0]);
    send(&mut d, &mut editor, "C-u 9 C-M-e");
    assert_eq!(cursors(&editor), [RUST.len()]);
    let said = messages(&editor);
    assert!(
        !said.iter().any(|message| message.ends_with("of buffer")),
        "{said:?}"
    );
    // A macro goes on past it, as Emacs's does.
    send(&mut d, &mut editor, "C-M-e C-f");
    assert_eq!(cursors(&editor), [RUST.len()]);
}

#[test]
fn the_mark_is_set_once_and_not_while_a_region_shows() {
    let (mut d, mut editor) = rust_at(&[offset("x();")]);
    send(&mut d, &mut editor, "C-M-a");
    assert_eq!(mark(&editor), Some(offset("x();")));
    assert_eq!(
        messages(&editor).last().map(String::as_str),
        Some("Mark set")
    );
    // Repeated, it keeps the mark where the first left it.
    send(&mut d, &mut editor, "C-M-a");
    assert_eq!(cursors(&editor), [offset("impl S")]);
    assert_eq!(mark(&editor), Some(offset("x();")));
    assert_eq!(
        editor.window().selections.mark_ring().iter().count(),
        0,
        "one mark, none in the ring"
    );
    // Another command between: the mark is set again.
    send(&mut d, &mut editor, "C-f C-M-e");
    assert_eq!(mark(&editor), Some(offset("impl S") + 1));
    // While a region shows, the motion grows it and sets no mark.
    place_at(&mut editor, &[offset("x();")]);
    send(&mut d, &mut editor, "C-SPC C-f C-M-a");
    assert_eq!(mark_active(&editor), MarkActive::Active);
    assert_eq!(mark(&editor), Some(offset("x();")));
    assert_eq!(cursors(&editor), [offset("fn one")]);
}

#[test]
fn shift_selects_from_where_the_cursors_were() {
    let (mut d, mut editor) = rust_at(&[offset("a: u32"), offset("x();")]);
    send(&mut d, &mut editor, "C-M-E");
    assert_eq!(mark_active(&editor), MarkActive::Shifted);
    assert_eq!(
        ranges(&editor),
        [
            offset("a: u32")..offset("\nimpl"),
            offset("x();")..offset("\n    fn two"),
        ]
    );
    send(&mut d, &mut editor, "C-f");
    assert_eq!(mark_active(&editor), MarkActive::Inactive);
}

#[test]
fn c_m_h_marks_the_defun_with_its_comment_and_again_the_next() {
    let (mut d, mut editor) = rust_at(&[offset("a: u32"), offset("x();")]);
    assert!(send(&mut d, &mut editor, "C-M-h").redraw);
    assert_eq!(mark_active(&editor), MarkActive::Active);
    assert_eq!(region(&editor), offset("// S.")..offset("\nimpl"));
    assert_eq!(cursors(&editor), [offset("// S.")]);
    // Again: the mark goes on past the next defun, and point stays.
    send(&mut d, &mut editor, "C-M-h");
    assert_eq!(region(&editor), offset("// S.")..RUST.len());
    // At the end, again: it stays, and says nothing.
    send(&mut d, &mut editor, "C-M-h");
    assert_eq!(region(&editor), offset("// S.")..RUST.len());
    assert!(!messages(&editor).iter().any(|said| said == "End of buffer"));
    // A count from a defun in an impl, and a negative one.
    place_at(&mut editor, &[offset("x();")]);
    send(&mut d, &mut editor, "C-g C-u 2 C-M-h");
    assert_eq!(
        region(&editor),
        offset("    fn one")..RUST.len() - "}\n".len()
    );
    place_at(&mut editor, &[offset("fn two")]);
    send(&mut d, &mut editor, "C-g M-- 2 C-M-h");
    assert_eq!(
        region(&editor),
        offset("    fn one")..RUST.len() - "}\n".len()
    );
    // A buffer with no defun.
    let (mut d, mut editor) = at("plain text\n", &[3]);
    send(&mut d, &mut editor, "C-M-h");
    assert_eq!(
        messages(&editor).last().map(String::as_str),
        Some("No defun here")
    );
    assert_eq!(mark(&editor), None);
}

#[test]
fn heads_in_a_highlighted_string_are_not_defuns() {
    let text = "const A: &str = r\"\nfn inside() {}\n\";\nfn b() {\n}\n";
    let (mut d, mut editor) = at(text, &[text.len()]);
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    assert!(buffer.set_mode(Mode::Rust));
    let string = "r\"\nfn inside() {}\n\"";
    let start = text.find(string).unwrap();
    let mut highlights = Highlights::new(text.len());
    highlights.replace(
        start..start + string.len(),
        [(string.len(), Some(Highlight::String))],
    );
    let revision = buffer.revision();
    assert!(buffer.take_highlights(revision, highlights));
    send(&mut d, &mut editor, "C-M-a C-M-a");
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn c_m_l_puts_the_defun_at_the_top_unless_point_would_leave() {
    // Ten rows: a defun with its comment, far down, and one longer than the window.
    let mut text = "x\n".repeat(30);
    text.push_str("// F.\nfn f() {\n    y();\n}\n");
    let long = text.len();
    text.push_str("fn g() {\n");
    text.push_str(&"    z();\n".repeat(20));
    text.push_str("}\n");
    let (mut d, mut editor) = at(&text, &[text.find("y();").unwrap()]);
    let id = editor.window().buffer();
    assert!(editor.buffer_mut(id).unwrap().set_mode(Mode::Rust));
    assert!(send(&mut d, &mut editor, "C-M-l").redraw);
    assert_eq!(editor.top_row(), 30, "the comment's line at the top");
    assert_eq!(cursors(&editor), [text.find("y();").unwrap()]);
    // Deep in a long defun: point's line at the bottom.
    let deep = long + "fn g() {\n".len() + 15 * "    z();\n".len();
    place_at(&mut editor, &[deep]);
    send(&mut d, &mut editor, "C-M-l");
    let row = 34 + 1 + 15;
    assert_eq!(editor.top_row(), row - 9);
    // No defun at or above point.
    place_at(&mut editor, &[4]);
    send(&mut d, &mut editor, "C-M-l");
    assert_eq!(
        messages(&editor).last().map(String::as_str),
        Some("No defun here")
    );
}
