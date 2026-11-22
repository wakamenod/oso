//! Tests of a native session's buffer in the editor (ADR 0165).
//!
//! Must not depend on: the host's threads, files or processes.

use crate::buffer::{Mode, ReadOnly};
use crate::editor::Editor;
use crate::selection::SelectionSet;
use crate::syntax::Highlight;

fn text(editor: &Editor, id: crate::buffer::BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

fn at(editor: &Editor, id: crate::buffer::BufferId, offset: usize) -> Option<Highlight> {
    editor.buffer(id).unwrap().highlights()?.at(offset)
}

#[test]
fn a_native_session_buffer_is_internal_read_only_and_withheld() {
    let mut editor = Editor::default();
    let id = editor.add_native_session_buffer("*claude: oso*");
    let buffer = editor.buffer(id).unwrap();
    assert!(buffer.is_native_session());
    assert_eq!(buffer.mode(), Mode::NativeSession);
    assert_eq!(buffer.mode().name(), "Session");
    assert_eq!(buffer.read_only(), Some(ReadOnly::Internal));
    assert!(buffer.withheld_from_agents());
    let other = editor.add_internal_buffer("*other*", Mode::Sessions);
    assert!(!editor.buffer(other).unwrap().is_native_session());
}

#[test]
fn rows_replace_their_bytes_and_keep_the_rest_highlighted() {
    let mut editor = Editor::default();
    let id = editor.add_native_session_buffer("*claude: oso*");
    editor.replace_native_rows(id, 0..0, "› hi\n", &[(4, Some(Highlight::CommentPerson))]);
    editor.replace_native_rows(id, 7..7, "● a\n", &[(3, Some(Highlight::SessionQuiet))]);
    assert_eq!(text(&editor, id), "› hi\n● a\n");
    assert_eq!(at(&editor, id, 0), Some(Highlight::CommentPerson));
    assert_eq!(at(&editor, id, 8), Some(Highlight::SessionQuiet));
    // The reply grows from its last line; the prompt keeps its runs.
    editor.replace_native_rows(
        id,
        7..13,
        "● ab\n  c\n",
        &[(4, Some(Highlight::SessionQuiet))],
    );
    assert_eq!(text(&editor, id), "› hi\n● ab\n  c\n");
    assert_eq!(at(&editor, id, 0), Some(Highlight::CommentPerson));
    assert_eq!(at(&editor, id, 7), Some(Highlight::SessionQuiet));
    assert_eq!(at(&editor, id, 14), None);
    let highlights = editor.buffer(id).unwrap().highlights().unwrap();
    assert_eq!(highlights.len(), text(&editor, id).len());
}

#[test]
fn a_range_is_clamped_to_the_text_and_its_characters() {
    let mut editor = Editor::default();
    let id = editor.add_native_session_buffer("*claude: oso*");
    editor.replace_native_rows(id, 0..0, "é\n", &[]);
    // Inside `é`, and past the end.
    editor.replace_native_rows(id, 1..99, "x\n", &[(1, Some(Highlight::SessionAlert))]);
    assert_eq!(text(&editor, id), "x\n");
    assert_eq!(at(&editor, id, 0), Some(Highlight::SessionAlert));
}

#[test]
fn another_buffer_takes_no_rows() {
    let mut editor = Editor::default();
    let other = editor.add_internal_buffer("*Spaces*", Mode::Sessions);
    editor.replace_native_rows(other, 0..0, "x\n", &[]);
    assert_eq!(text(&editor, other), "");
}

#[test]
fn a_cursor_at_the_end_follows_new_rows() {
    let mut editor = Editor::default();
    let id = editor.add_native_session_buffer("*claude: oso*");
    editor.show(id);
    editor.replace_native_rows(id, 0..0, "one\n", &[]);
    let end = text(&editor, id).len();
    editor.current_mut().0.selections = SelectionSet::new(end);
    editor.replace_native_rows(id, end..end, "two\n", &[]);
    assert_eq!(
        editor.window().selections.primary().cursor,
        text(&editor, id).len()
    );
    // A cursor moved away stays where it was.
    editor.current_mut().0.selections = SelectionSet::new(1);
    let end = text(&editor, id).len();
    editor.replace_native_rows(id, end..end, "three\n", &[]);
    assert_eq!(editor.window().selections.primary().cursor, 1);
}

#[test]
fn a_native_session_takes_the_session_flags() {
    let mut editor = Editor::default();
    let id = editor.add_native_session_buffer("*claude: oso*");
    editor.set_hosts_session(id, true);
    editor.set_running(id, true);
    assert_eq!(editor.busy_native_sessions().count(), 0);
    editor.set_session_turn(id, true);
    assert_eq!(editor.busy_native_sessions().collect::<Vec<_>>(), vec![id]);
    assert!(editor.buffer(id).unwrap().hosts_session());
    editor.set_agent_reads(id, true);
    assert!(editor.buffer(id).unwrap().agent_reads());
    // Not a terminal: none of it makes it one.
    assert_eq!(editor.busy_terminals().count(), 0);
    assert_eq!(editor.running_terminals().count(), 0);
    let other = editor.add_internal_buffer("*Spaces*", Mode::Sessions);
    editor.set_session_turn(other, true);
    editor.set_running(other, true);
    assert!(!editor.buffer(other).unwrap().is_busy());
}
