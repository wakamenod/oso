//! Tests of edits through [`Editor`]: text and cursors in every window, read-only
//! refusal and the helpers of the other editing tests (ADR 0053).
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written
//! out here.

use super::*;
use crate::display_map::TabWidth;
use crate::selection::Selection;

pub(super) fn edit(range: std::ops::Range<usize>, text: &str) -> Replacement<'_> {
    Replacement { range, text }
}

pub(super) fn editor_with(text: &str) -> (Editor, BufferId) {
    let editor = Editor::with_buffer(Buffer::from_text("t", text));
    let id = editor.window().buffer();
    (editor, id)
}

pub(super) fn text(editor: &Editor, id: BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

pub(super) fn set_at(cursors: &[usize]) -> SelectionSet {
    let parts = cursors.iter().copied().map(Selection::at).collect();
    SelectionSet::from_parts(parts, 0).unwrap()
}

pub(super) fn cursors(window: &Window) -> Vec<usize> {
    window.selections.iter().map(|s| s.cursor).collect()
}

/// Adds a window showing `id`, not selected.
pub(super) fn second_window(editor: &mut Editor, id: BufferId, at: &[usize]) -> usize {
    let mut window = Window::new(id);
    window.selections = set_at(at);
    editor.windows.push(window);
    editor.windows.len() - 1
}

/// One command that inserts `typed` at every cursor, as the dispatcher and a
/// self-insert command will run it.
pub(super) fn self_insert(editor: &mut Editor, typed: &str) {
    editor.begin_undo_group();
    editor.allow_undo_join();
    let id = editor.window().buffer();
    let edits: Vec<_> = editor
        .window()
        .selections
        .iter()
        .map(|s| edit(s.cursor..s.cursor, typed))
        .collect();
    editor.apply_edits(id, &edits).unwrap();
    editor.end_undo_group();
}

pub(super) fn plain_edit(
    editor: &mut Editor,
    id: BufferId,
    range: std::ops::Range<usize>,
    new: &str,
) {
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit(range, new)]).unwrap();
    editor.end_undo_group();
}

#[test]
fn an_edit_changes_text_and_display_and_moves_the_cursor_after_it() {
    let (mut editor, id) = editor_with("ab");
    editor.windows[0].selections = SelectionSet::new(1);
    let revision = editor.buffer(id).unwrap().revision();
    editor.apply_edits(id, &[edit(1..1, "日本")]).unwrap();
    assert_eq!(text(&editor, id), "a日本b");
    assert_eq!(cursors(&editor.windows[0]), [7]);
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.revision(), revision + 1);
    assert_eq!(buffer.display().max_point().column, 6);
}

#[test]
fn a_second_window_follows_as_a_marker() {
    let (mut editor, id) = editor_with("0123456789");
    editor.windows[0].selections = SelectionSet::new(5);
    let other = second_window(&mut editor, id, &[2, 5, 8]);
    editor.apply_edits(id, &[edit(5..5, "xx")]).unwrap();
    assert_eq!(cursors(&editor.windows[0]), [7], "after the new text");
    assert_eq!(
        cursors(&editor.windows[other]),
        [2, 5, 10],
        "at the insertion it stays before the new text"
    );
}

#[test]
fn a_window_on_another_buffer_is_left_alone() {
    let (mut editor, id) = editor_with("abc");
    let other_buffer = editor.add_buffer(Buffer::from_text("o", "xyz"));
    let other = second_window(&mut editor, other_buffer, &[2]);
    editor.apply_edits(id, &[edit(0..0, "q")]).unwrap();
    assert_eq!(cursors(&editor.windows[other]), [2]);
    assert_eq!(text(&editor, other_buffer), "xyz");
}

#[test]
fn editing_a_buffer_the_selected_window_does_not_show_moves_only_its_windows() {
    let (mut editor, id) = editor_with("abc");
    let other_buffer = editor.add_buffer(Buffer::from_text("o", "xyz"));
    let other = second_window(&mut editor, other_buffer, &[1]);
    editor.windows[0].selections = SelectionSet::new(1);
    editor
        .apply_edits(other_buffer, &[edit(1..1, "Q")])
        .unwrap();
    assert_eq!(cursors(&editor.windows[0]), [1]);
    assert_eq!(cursors(&editor.windows[other]), [1], "not selected: stays");
    assert_eq!(text(&editor, id), "abc");
    // With no cursors in the record, the undo in that buffer maps them.
    editor.selected = other;
    editor.undo().unwrap();
    assert_eq!(text(&editor, other_buffer), "xyz");
    assert_eq!(cursors(&editor.windows[other]), [1]);
}

#[test]
fn cursors_clip_to_display_clusters_after_a_combining_mark() {
    let (mut editor, id) = editor_with("e");
    let other = second_window(&mut editor, id, &[1]);
    editor.apply_edits(id, &[edit(1..1, "\u{301}")]).unwrap();
    assert_eq!(text(&editor, id), "e\u{301}");
    // Offset 1 now lies inside the cluster, so the cursor goes to its start.
    assert_eq!(cursors(&editor.windows[other]), [0]);
}

#[test]
fn cursors_that_meet_merge_and_a_primary_remains() {
    let (mut editor, id) = editor_with("0123456789");
    editor.windows[0].selections = SelectionSet::from_parts(
        vec![Selection::at(2), Selection::at(4), Selection::at(8)],
        2,
    )
    .unwrap();
    editor.apply_edits(id, &[edit(1..6, "")]).unwrap();
    assert_eq!(cursors(&editor.windows[0]), [1, 3]);
    assert_eq!(editor.window().selections.primary().cursor, 3);
}

#[test]
fn a_multi_cursor_edit_applies_at_each_cursor() {
    let (mut editor, id) = editor_with("a\nb\nc\n");
    editor.windows[0].selections = set_at(&[1, 3, 5]);
    self_insert(&mut editor, "日");
    assert_eq!(text(&editor, id), "a日\nb日\nc日\n");
    assert_eq!(cursors(&editor.windows[0]), [4, 9, 14]);
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "a\nb\nc\n");
    assert_eq!(cursors(&editor.windows[0]), [1, 3, 5]);
    editor.redo().unwrap();
    assert_eq!(text(&editor, id), "a日\nb日\nc日\n");
    assert_eq!(cursors(&editor.windows[0]), [4, 9, 14]);
}

#[test]
fn read_only_buffers_refuse_edits_and_nothing_changes() {
    let bad = Buffer::from_bytes("bad", b"a\xff");
    let mut editor = Editor::with_buffer(bad);
    let id = editor.window().buffer();
    let revision = editor.buffer(id).unwrap().revision();
    assert_eq!(
        editor.apply_edits(id, &[edit(0..0, "x")]),
        Err(EditError::ReadOnly(crate::buffer::ReadOnly::InvalidUtf8))
    );
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.revision(), revision);
    assert_eq!(buffer.invalid_bytes().len(), 1);
    assert!(!buffer.is_modified());
    assert_eq!(buffer.undo_lens(), (0, 0));
    assert_eq!(
        editor.undo(),
        Err(EditError::ReadOnly(crate::buffer::ReadOnly::InvalidUtf8))
    );
}

#[test]
fn a_load_with_invalid_bytes_ends_undo_and_redo() {
    let (mut editor, id) = editor_with("abc");
    plain_edit(&mut editor, id, 0..0, "x");
    plain_edit(&mut editor, id, 0..0, "y");
    editor.undo().unwrap();
    let mut builder = crate::load::Builder::new(None, TabWidth::default());
    builder.push(b"a\xff");
    builder.finish();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.apply_load(builder.snapshot());
    let refused = EditError::ReadOnly(crate::buffer::ReadOnly::InvalidUtf8);
    assert_eq!(editor.undo(), Err(refused));
    assert_eq!(editor.redo(), Err(refused));
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.invalid_bytes().len(), 1);
    assert_eq!(buffer.undo_lens(), (0, 0));
}

#[test]
fn extreme_ranges_clamp_and_undo() {
    let (mut editor, id) = editor_with("日本語");
    editor.windows[0].selections = SelectionSet::new(3);
    #[expect(clippy::reversed_empty_ranges, reason = "what the test is about")]
    let reversed = usize::MAX - 1..1;
    for range in [usize::MAX..usize::MAX, 0..usize::MAX, reversed] {
        plain_edit(&mut editor, id, range, "x");
    }
    assert_eq!(text(&editor, id), "xx");
    while editor.undo().is_ok() {}
    assert_eq!(text(&editor, id), "日本語");
    assert_eq!(cursors(&editor.windows[0]), [3]);
    while editor.redo().is_ok() {}
    assert_eq!(text(&editor, id), "xx");
}

#[test]
fn an_unknown_buffer_is_an_error() {
    let (mut editor, _) = editor_with("");
    assert_eq!(
        editor.apply_edits(BufferId(u64::MAX), &[edit(0..0, "x")]),
        Err(EditError::NoSuchBuffer)
    );
}

#[test]
fn empty_edits_leave_no_trace() {
    let (mut editor, id) = editor_with("abc");
    let buffer = editor.buffer(id).unwrap();
    let (revision, content) = (buffer.revision(), buffer.content_id());
    editor.apply_edits(id, &[edit(1..1, "")]).unwrap();
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.revision(), revision);
    assert_eq!(buffer.content_id(), content);
    assert_eq!(editor.undo(), Err(EditError::NoUndo));
}
