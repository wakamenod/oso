//! The tests of a file read again after it changed on disk (ADR 0083). The host's side, which
//! notices the change and reads the file, is tested in the tip crate.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;

use super::{Editor, ReloadOutcome, Status};
use crate::buffer::{Buffer, BufferId, ContentId, Replacement};
use crate::display_map::TabWidth;
use crate::load::{Reloaded, decode_all};
use crate::rope::Rope;
use crate::selection::{Selection, SelectionSet};
use crate::window::{Scroll, ViewSize};

fn path() -> PathBuf {
    PathBuf::from("/nowhere/notes.txt")
}

fn visiting(content: &str) -> (Editor, BufferId) {
    let mut buffer = Buffer::visiting(path());
    buffer.judge_secret(Some(&[path()]));
    buffer.apply_load(decode_all(content.as_bytes(), TabWidth::default()));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(5, 20));
    let id = editor.window().buffer();
    (editor, id)
}

/// `new` read again, compared with what `id` holds now, as the host's thread does.
fn read_again(editor: &Editor, id: BufferId, new: &[u8]) -> (Reloaded, ContentId) {
    let buffer = editor.buffer(id).unwrap();
    let old: Rope = buffer.text().clone();
    let reloaded = decode_all(new, TabWidth::default()).compared_with(&old);
    (reloaded, buffer.content_id())
}

fn reload(editor: &mut Editor, id: BufferId, new: &str) -> ReloadOutcome {
    let (reloaded, from) = read_again(editor, id, new.as_bytes());
    editor.reload(id, reloaded, from)
}

fn text(editor: &Editor, id: BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

fn modified(editor: &Editor, id: BufferId) -> bool {
    editor.buffer(id).unwrap().is_modified()
}

fn type_at(editor: &mut Editor, id: BufferId, offset: usize, text: &str) {
    let edit = Replacement {
        range: offset..offset,
        text,
    };
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit]).unwrap();
    editor.end_undo_group();
}

fn set_cursors(editor: &mut Editor, offsets: &[usize]) {
    let selections = offsets
        .iter()
        .map(|&offset| Selection::at(offset))
        .collect();
    editor.current_mut().0.selections = SelectionSet::from_parts(selections, 0).unwrap();
}

fn cursors(editor: &Editor) -> Vec<usize> {
    editor
        .window()
        .selections
        .iter()
        .map(|selection| selection.cursor)
        .collect()
}

fn warnings(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(&mut status);
    status.warnings
}

#[test]
fn a_reload_is_one_undo_step_and_leaves_the_buffer_unmodified() {
    let (mut editor, id) = visiting("one\ntwo\n");
    assert_eq!(reload(&mut editor, id, "one\n2\n"), ReloadOutcome::Reloaded);
    assert_eq!(text(&editor, id), "one\n2\n");
    assert!(!modified(&editor, id), "it is what the disk holds");
    assert_eq!(editor.buffer(id).unwrap().display().row_count(), 3);

    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "one\ntwo\n");
    assert!(modified(&editor, id), "the disk holds the new text now");
    editor.redo().unwrap();
    assert_eq!(text(&editor, id), "one\n2\n");
    assert!(!modified(&editor, id));
}

#[test]
fn edits_before_a_reload_undo_after_it() {
    let (mut editor, id) = visiting("abc\n");
    type_at(&mut editor, id, 0, "x");
    editor.undo().unwrap();
    assert!(!modified(&editor, id));
    assert_eq!(reload(&mut editor, id, "abcd\n"), ReloadOutcome::Reloaded);
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "abc\n");
    editor.redo().unwrap();
    assert_eq!(text(&editor, id), "abcd\n");
    assert!(
        editor.redo().is_err(),
        "the reload ended the redo of the typing"
    );
}

#[test]
fn cursors_keep_their_place_around_the_change_and_their_line_inside_it() {
    let (mut editor, id) = visiting("head\nold line\nmiddle\ntail\n");
    // In the head, in the changed lines, and in the tail.
    set_cursors(&mut editor, &[2, 7, 14, 24]);
    let outcome = reload(&mut editor, id, "head\nnew text here\nmid\ntail\n");
    assert_eq!(outcome, ReloadOutcome::Reloaded);
    // "head" stays; 7 is line 1 column 2 and 14 is line 2 column 0, so they stay on their
    // lines; the tail moves with the text before it.
    assert_eq!(cursors(&editor), vec![2, 7, 19, 26]);
}

#[test]
fn a_cursor_past_a_shorter_line_goes_to_its_end() {
    let (mut editor, id) = visiting("a\nlong line\nz\n");
    set_cursors(&mut editor, &[11]);
    reload(&mut editor, id, "a\nab\nz\n");
    assert_eq!(cursors(&editor), vec![4]);
}

#[test]
fn the_view_keeps_its_top_row() {
    let content: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let (mut editor, id) = visiting(&content);
    let top = content.find("line 20").unwrap();
    editor.restore_positions(SelectionSet::new(top), Scroll { top: 20, left: 0 });
    let changed = content.replace("line 3\n", "line three\n");
    reload(&mut editor, id, &changed);
    assert_eq!(editor.window().scroll().top, 20);
    assert_eq!(cursors(&editor), vec![top + 4]);
}

#[test]
fn a_buffer_with_unsaved_edits_keeps_them_and_warns() {
    let (mut editor, id) = visiting("abc\n");
    type_at(&mut editor, id, 0, "x");
    assert_eq!(reload(&mut editor, id, "new\n"), ReloadOutcome::Edited);
    assert_eq!(text(&editor, id), "xabc\n");
    assert_eq!(warnings(&editor), "[changed on disk] ");
}

#[test]
fn a_buffer_changed_while_the_file_was_read_keeps_its_text() {
    let (mut editor, id) = visiting("abc\n");
    let (reloaded, from) = read_again(&editor, id, b"new\n");
    type_at(&mut editor, id, 0, "x");
    // Saved meanwhile, so unmodified, but not what the read compared with.
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.mark_saved(buffer.content_id());
    assert_eq!(editor.reload(id, reloaded, from), ReloadOutcome::Edited);
    assert_eq!(text(&editor, id), "xabc\n");
}

#[test]
fn a_buffer_undone_to_what_was_read_takes_the_reload() {
    let (mut editor, id) = visiting("abc\n");
    let (reloaded, from) = read_again(&editor, id, b"new\n");
    type_at(&mut editor, id, 0, "x");
    editor.undo().unwrap();
    assert_eq!(editor.reload(id, reloaded, from), ReloadOutcome::Reloaded);
    assert_eq!(text(&editor, id), "new\n");
}

#[test]
fn a_reload_waits_while_a_prompt_is_open() {
    let (mut editor, id) = visiting("abc\n");
    editor.ask("Really", "ignore").unwrap();
    assert!(editor.reload_waits());
    assert_eq!(reload(&mut editor, id, "new\n"), ReloadOutcome::Later);
    assert_eq!(text(&editor, id), "abc\n");
}

#[test]
fn the_same_text_changes_nothing() {
    let (mut editor, id) = visiting("abc\n");
    let revision = editor.buffer(id).unwrap().revision();
    assert_eq!(reload(&mut editor, id, "abc\n"), ReloadOutcome::Same);
    assert_eq!(editor.buffer(id).unwrap().revision(), revision);
    assert!(editor.undo().is_err(), "nothing to undo");
}

#[test]
fn a_binary_file_replaces_the_text_with_no_undo() {
    let (mut editor, id) = visiting("abc\n");
    let (reloaded, from) = read_again(&editor, id, b"a\0b");
    assert_eq!(editor.reload(id, reloaded, from), ReloadOutcome::Reloaded);
    assert_eq!(text(&editor, id), "a\0b");
    assert!(!modified(&editor, id));
    assert!(editor.undo().is_err());
}

#[test]
fn a_file_gone_warns_until_it_is_back() {
    let (mut editor, id) = visiting("abc\n");
    editor.file_gone_from_disk(id);
    assert_eq!(warnings(&editor), "[deleted on disk] ");
    type_at(&mut editor, id, 0, "x");
    editor.file_changed_on_disk(id);
    assert_eq!(warnings(&editor), "[deleted on disk] ", "one of the two");
    editor.undo().unwrap();
    assert_eq!(reload(&mut editor, id, "back\n"), ReloadOutcome::Reloaded);
    assert_eq!(warnings(&editor), "");
}

#[test]
fn a_reload_never_withholds_less() {
    let (mut editor, id) = visiting("abc\n");
    assert!(!editor.buffer(id).unwrap().is_secret());
    let secret = [PathBuf::from("/home/me/.ssh/id_ed25519")];
    editor.judge_reload(id, Ok(&secret));
    assert!(editor.buffer(id).unwrap().is_secret());
    editor.judge_reload(id, Ok(&[path()]));
    assert!(
        editor.buffer(id).unwrap().is_secret(),
        "undo can bring back what was read under it"
    );
}

#[test]
fn a_file_nobody_could_resolve_is_withheld() {
    let (mut editor, id) = visiting("abc\n");
    editor.judge_reload(id, Err(&[]));
    assert!(editor.buffer(id).unwrap().is_secret());
}

#[test]
fn a_revert_discards_edits_as_one_undo_step() {
    let (mut editor, id) = visiting("abc\n");
    type_at(&mut editor, id, 0, "x");
    editor.request_revert();
    assert_eq!(editor.take_revert_requests(), vec![id]);
    assert!(editor.take_revert_requests().is_empty());
    let (reloaded, from) = read_again(&editor, id, b"disk\n");
    assert_eq!(editor.revert(id, reloaded, from), ReloadOutcome::Reloaded);
    assert_eq!(text(&editor, id), "disk\n");
    assert!(!modified(&editor, id));
    editor.undo().unwrap();
    assert_eq!(text(&editor, id), "xabc\n", "the edits are one undo away");
}

#[test]
fn a_revert_to_the_same_text_leaves_the_buffer_unmodified() {
    let (mut editor, id) = visiting("abc\n");
    type_at(&mut editor, id, 0, "x");
    let edit = Replacement {
        range: 0..1,
        text: "",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    assert!(modified(&editor, id), "typed and deleted");
    let (reloaded, from) = read_again(&editor, id, b"abc\n");
    assert_eq!(editor.revert(id, reloaded, from), ReloadOutcome::Same);
    assert!(!modified(&editor, id));
}
