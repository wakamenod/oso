//! Tests of what the completion popup takes after it opened (ADR 0138, step 9b): an item's
//! edits besides its own in its insert's undo step, from the answer or its resolve, before the
//! insert or after it, dropped once the text changed, the line out of view said, the resolve
//! asked once a selection, and what the person typed.
//!
//! Must not depend on: the host or its servers; these drive the editor alone.

use std::ops::Range;

use super::super::super::super::Editor;
use crate::buffer::Buffer;
use crate::editor::edit_tests::edit;
use crate::editor::{CompletionPopupAnswer, CompletionPopupItem};
use crate::frame::FrameSize;
use crate::selection::SelectionSet;

fn editing(text: &str, at: usize) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", text));
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 80,
    });
    editor.current_mut().0.selections = SelectionSet::new(at);
    editor
}

fn item(label: &str, extra: Vec<(Range<usize>, Box<str>)>) -> CompletionPopupItem {
    CompletionPopupItem {
        label: label.into(),
        annotation: "".into(),
        filter: None,
        text: label.into(),
        range: None,
        extra,
    }
}

fn open(editor: &mut Editor, items: Vec<CompletionPopupItem>) {
    let at = editor.ask_at();
    let answer = CompletionPopupAnswer {
        serial: 9,
        incomplete: false,
        at,
        revision: at.revision,
        items,
    };
    editor.open_completion_popup(answer).unwrap();
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn use_line() -> Vec<(Range<usize>, String)> {
    vec![(0..0, "use a::fo;\n".to_owned())]
}

#[test]
fn an_answer_s_extra_edits_go_in_with_the_item_in_one_undo_step() {
    let mut editor = editing("fn f() {}\nfo", 12);
    open(
        &mut editor,
        vec![item("foo", vec![(0..0, "use a::foo;\n".into())])],
    );
    editor.begin_undo_group();
    editor.completion_popup_insert().unwrap();
    editor.end_undo_group();
    assert_eq!(text(&editor), "use a::foo;\nfn f() {}\nfoo");
    editor.undo().unwrap();
    assert_eq!(text(&editor), "fn f() {}\nfo", "one step");
}

#[test]
fn the_selected_item_is_resolved_once_and_its_edits_kept_with_it() {
    let mut editor = editing("x\nfo", 4);
    open(
        &mut editor,
        vec![item("foo", Vec::new()), item("form", Vec::new())],
    );
    assert_eq!(editor.take_completion_resolve(), Some((9, 0)));
    assert_eq!(editor.take_completion_resolve(), None, "once");
    editor.completion_popup_move(1);
    assert_eq!(editor.take_completion_resolve(), Some((9, 1)));
    let revision = editor.current().1.revision();
    assert!(!editor.completion_resolved((9, 1), revision, use_line()));
    assert!(
        !editor.completion_resolved((8, 1), revision, use_line()),
        "another answer's"
    );
    editor.completion_popup_insert().unwrap();
    assert_eq!(text(&editor), "use a::fo;\nx\nform");
}

#[test]
fn edits_resolved_after_the_insert_join_its_step_unless_the_text_changed() {
    let mut editor = editing("x\nfo", 4);
    open(&mut editor, vec![item("foo", Vec::new())]);
    assert!(editor.take_completion_resolve().is_some());
    editor.begin_undo_group();
    editor.completion_popup_insert().unwrap();
    editor.end_undo_group();
    let revision = editor.current().1.revision();
    assert!(editor.completion_resolved((9, 0), revision, use_line()));
    assert_eq!(text(&editor), "use a::fo;\nx\nfoo");
    editor.undo().unwrap();
    assert_eq!(
        text(&editor),
        "x\nfo",
        "the insert and its edits undo as one"
    );
    // Once the text changed, they are dropped.
    let mut editor = editing("x\nfo", 4);
    open(&mut editor, vec![item("foo", Vec::new())]);
    assert!(editor.take_completion_resolve().is_some());
    editor.completion_popup_insert().unwrap();
    let revision = editor.current().1.revision();
    let id = editor.window().buffer();
    editor.apply_edits(id, &[edit(7..7, "x")]).unwrap();
    assert!(!editor.completion_resolved((9, 0), revision, use_line()));
    assert_eq!(text(&editor), "x\nfoox");
    let echo = editor.echo_text().to_owned();
    assert!(echo.contains("not made"), "{echo}");
}

#[test]
fn an_extra_edit_out_of_view_says_which_line_it_changed() {
    let text = format!("fn f() {{}}\n{}fo", "\n".repeat(60));
    let at = text.len();
    let mut editor = editing(&text, at);
    editor.follow_cursor();
    open(
        &mut editor,
        vec![item("foo", vec![(0..0, "use a::foo;\n".into())])],
    );
    editor.completion_popup_insert().unwrap();
    let echo = editor.echo_text().to_owned();
    assert!(echo.contains("line 1"), "{echo}");
}

#[test]
fn what_the_person_typed_is_noted_with_the_word_before_the_cursor() {
    let mut editor = editing("let x = foo.", 12);
    editor.note_completion_typed();
    let typed = editor.take_completion_typed().unwrap();
    assert_eq!((typed.last, typed.word), ('.', 0));
    let mut editor = editing("let x = fooé", 13);
    editor.note_completion_typed();
    let typed = editor.take_completion_typed().unwrap();
    assert_eq!((typed.last, typed.word), ('é', 4));
    assert!(editor.take_completion_typed().is_none(), "once");
}
