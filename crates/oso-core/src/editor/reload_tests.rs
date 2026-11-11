//! The tests of a file read again after it changed on disk (ADR 0083). The host's side, which
//! notices the change and reads the file, is tested in the tip crate.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::{Editor, ReloadOutcome, Status};
use crate::buffer::{Baseline, Buffer, BufferId, ContentId, DiskState, FileStamp, Replacement};
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
    editor.reload(id, reloaded, from).0
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
    editor.status_into(editor.tiled().0.id(), &mut status);
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
    editor.restore_positions(
        SelectionSet::new(top),
        Scroll {
            start: top,
            left: 0,
        },
    );
    let changed = content.replace("line 3\n", "line three\n");
    reload(&mut editor, id, &changed);
    assert_eq!(editor.top_row(), 20);
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
    assert_eq!(editor.reload(id, reloaded, from).0, ReloadOutcome::Edited);
    assert_eq!(text(&editor, id), "xabc\n");
}

#[test]
fn a_buffer_undone_to_what_was_read_takes_the_reload() {
    let (mut editor, id) = visiting("abc\n");
    let (reloaded, from) = read_again(&editor, id, b"new\n");
    type_at(&mut editor, id, 0, "x");
    editor.undo().unwrap();
    assert_eq!(editor.reload(id, reloaded, from).0, ReloadOutcome::Reloaded);
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
fn a_reload_waits_while_an_isearch_is_on() {
    let (mut editor, id) = visiting("abc\n");
    editor.isearch_begin(false);
    editor.isearch_add("b");
    assert!(editor.reload_waits());
    assert_eq!(reload(&mut editor, id, "new\n"), ReloadOutcome::Later);
    assert_eq!(text(&editor, id), "abc\n");
    editor.isearch_exit();
    assert!(!editor.reload_waits());
    assert_eq!(reload(&mut editor, id, "new\n"), ReloadOutcome::Reloaded);
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
    assert_eq!(editor.reload(id, reloaded, from).0, ReloadOutcome::Reloaded);
    assert_eq!(text(&editor, id), "a\0b");
    assert!(!modified(&editor, id));
    assert!(editor.undo().is_err());
}

fn stamp(len: u64) -> FileStamp {
    FileStamp {
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(7),
        len,
        mode: 0o644,
        identity: Some((1, 2)),
    }
}

/// `id` visiting a file read as `stamp(len)`.
fn with_baseline(editor: &mut Editor, id: BufferId, len: u64) {
    editor.buffer_mut(id).unwrap().set_baseline(Baseline {
        location: path(),
        file: Some(stamp(len)),
    });
}

fn seen(editor: &mut Editor, id: BufferId, state: DiskState) -> Vec<BufferId> {
    editor.files_seen(&[(id, state)], false, |_| false)
}

#[test]
fn a_file_as_read_needs_nothing_and_a_changed_one_a_read_once() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    assert!(seen(&mut editor, id, DiskState::File(stamp(4))).is_empty());
    let changed = DiskState::File(stamp(9));
    assert_eq!(seen(&mut editor, id, changed), vec![id]);
    assert!(seen(&mut editor, id, changed).is_empty(), "once");
    assert!(
        editor
            .files_seen(&[(id, changed)], false, |_| true)
            .is_empty(),
        "nor while a read runs"
    );
}

#[test]
fn another_file_of_the_same_time_and_size_is_a_change() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    let swapped = FileStamp {
        identity: Some((1, 3)),
        ..stamp(4)
    };
    assert_eq!(seen(&mut editor, id, DiskState::File(swapped)), vec![id]);
}

#[test]
fn a_file_changed_under_edits_warns_once_and_may_be_saved_over() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    type_at(&mut editor, id, 0, "x");
    let changed = DiskState::File(stamp(9));
    assert!(seen(&mut editor, id, changed).is_empty());
    assert_eq!(warnings(&editor), "[changed on disk] ");
    assert_eq!(
        editor.buffer(id).unwrap().save_over(),
        Some(stamp(9)),
        "a save asks before it goes over this file"
    );
    seen(&mut editor, id, changed);
    let messages = editor
        .buffer(editor.messages_buffer())
        .unwrap()
        .text()
        .to_string();
    assert_eq!(messages.matches("changed on disk").count(), 1, "{messages}");
    // Undone to what was read: it is read again.
    editor.undo().unwrap();
    assert_eq!(seen(&mut editor, id, changed), vec![id]);
}

#[test]
fn a_file_gone_warns_until_it_is_back() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    type_at(&mut editor, id, 0, "x");
    seen(&mut editor, id, DiskState::Gone);
    assert_eq!(warnings(&editor), "[deleted on disk] ");
    // Back as something else, under the edits.
    seen(&mut editor, id, DiskState::File(stamp(9)));
    assert_eq!(warnings(&editor), "[changed on disk] ");
    seen(&mut editor, id, DiskState::Gone);
    // Back as it was read.
    seen(&mut editor, id, DiskState::File(stamp(4)));
    assert_eq!(warnings(&editor), "");
}

#[test]
fn a_copy_put_in_for_a_file_gone_is_not_told_again() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.take_format_of_copy(false, crate::decode::LineEnding::Lf, true);
    seen(&mut editor, id, DiskState::Gone);
    assert_eq!(warnings(&editor), "");
}

#[test]
fn a_chmod_alone_is_taken_in() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    let chmod = FileStamp {
        mode: 0o600,
        ..stamp(4)
    };
    assert!(seen(&mut editor, id, DiskState::File(chmod)).is_empty());
    let read = editor.buffer(id).unwrap().baseline().unwrap().file.unwrap();
    assert_eq!(read.mode, 0o600);
}

#[test]
fn nothing_is_acted_on_while_a_prompt_is_open() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    editor.ask("Really", "ignore").unwrap();
    assert!(seen(&mut editor, id, DiskState::File(stamp(9))).is_empty());
}

#[test]
fn a_revert_that_must_wait_is_asked_for_again() {
    let (mut editor, id) = visiting("abc\n");
    type_at(&mut editor, id, 0, "x");
    let (reloaded, from) = read_again(&editor, id, b"disk\n");
    editor.ask("Really", "ignore").unwrap();
    assert_eq!(editor.revert(id, reloaded, from).0, ReloadOutcome::Later);
    assert!(
        editor.take_revert_requests().is_empty(),
        "not while the prompt is open"
    );
    editor.quit();
    assert_eq!(editor.take_revert_requests(), vec![(id, from)]);
}

#[test]
fn a_revert_asked_for_drops_what_was_typed_after_only_by_asking_again() {
    let (mut editor, id) = visiting("abc\n");
    type_at(&mut editor, id, 0, "x");
    editor.request_revert();
    type_at(&mut editor, id, 0, "y");
    assert!(editor.take_revert_requests().is_empty());
    let messages = editor
        .buffer(editor.messages_buffer())
        .unwrap()
        .text()
        .to_string();
    assert!(
        messages.contains("Not reloaded, as notes.txt changed meanwhile"),
        "{messages}"
    );
    assert_eq!(text(&editor, id), "yxabc\n");
}

#[test]
fn a_revert_to_a_binary_file_keeps_the_edits() {
    let (mut editor, id) = visiting("abc\n");
    type_at(&mut editor, id, 0, "x");
    let (reloaded, from) = read_again(&editor, id, b"a\0b");
    assert_eq!(
        editor.revert(id, reloaded, from).0,
        ReloadOutcome::NotEditable
    );
    assert_eq!(text(&editor, id), "xabc\n");
    assert!(modified(&editor, id));
}

#[test]
fn a_cursor_on_a_line_gone_stays_in_the_change() {
    let (mut editor, id) = visiting("x\n\n\nY");
    set_cursors(&mut editor, &[3]);
    reload(&mut editor, id, "x\nY");
    assert_eq!(cursors(&editor), vec![2], "before the Y, not after it");
}

#[test]
fn a_read_that_failed_warns_and_is_tried_again_at_a_new_look() {
    let (mut editor, id) = visiting("abc\n");
    with_baseline(&mut editor, id, 4);
    let changed = DiskState::File(stamp(9));
    assert_eq!(seen(&mut editor, id, changed), vec![id]);
    editor.reload_failed(id);
    assert_eq!(warnings(&editor), "[changed on disk] ", "with no edits too");
    assert!(
        seen(&mut editor, id, changed).is_empty(),
        "not at every check"
    );
    assert_eq!(
        editor.files_seen(&[(id, changed)], true, |_| false),
        vec![id],
        "but at a new look"
    );
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
    let from = editor.buffer(id).unwrap().content_id();
    assert_eq!(editor.take_revert_requests(), vec![(id, from)]);
    assert!(editor.take_revert_requests().is_empty());
    let (reloaded, from) = read_again(&editor, id, b"disk\n");
    assert_eq!(editor.revert(id, reloaded, from).0, ReloadOutcome::Reloaded);
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
    assert_eq!(editor.revert(id, reloaded, from).0, ReloadOutcome::Same);
    assert!(!modified(&editor, id));
}

proptest::proptest! {
    #[test]
    fn cursors_stay_on_characters_in_the_new_text(
        old in "[aé\n🍣\u{301}]{0,40}",
        new in "[aé\n🍣\u{301}]{0,40}",
        offsets in proptest::collection::vec(0usize..200, 1..4),
    ) {
        let (mut editor, id) = visiting(&old);
        let offsets: Vec<usize> = offsets.iter().map(|o| (*o).min(old.len())).collect();
        let selections = offsets
            .iter()
            .map(|&offset| {
                let offset = editor.buffer(id).unwrap().text().clip_offset(offset, crate::sum_tree::Bias::Left);
                Selection::at(offset)
            })
            .collect();
        editor.current_mut().0.selections = SelectionSet::from_parts(selections, 0).unwrap();
        reload(&mut editor, id, &new);
        proptest::prop_assert_eq!(text(&editor, id), new.clone());
        for cursor in cursors(&editor) {
            proptest::prop_assert!(cursor <= new.len() && new.is_char_boundary(cursor));
        }
    }
}

#[test]
fn a_wrapped_buffer_read_again_wraps_its_new_text() {
    use super::wrap_tests::{fresh, rows, settle};
    let (mut editor, id) = visiting(&format!("{}\nend\n", "word ".repeat(60)));
    assert!(editor.set_wraps(id, true));
    settle(&mut editor);
    reload(&mut editor, id, &format!("{}\nend\n", "word ".repeat(70)));
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
}

#[test]
fn a_change_around_the_view_keeps_its_top_row() {
    let content: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let (mut editor, id) = visiting(&content);
    let top = content.find("line 20").unwrap();
    let cursor = content.find("line 22").unwrap();
    editor.restore_positions(
        SelectionSet::new(cursor),
        Scroll {
            start: top,
            left: 0,
        },
    );
    assert_eq!(editor.top_row(), 20);
    // One change from line 3 to line 35, as two places changed on disk make.
    let changed = content
        .replace("line 3\n", "line three\n")
        .replace("line 35\n", "line thirty-five\n");
    reload(&mut editor, id, &changed);
    assert_eq!(editor.top_row(), 20, "the top keeps its line");
    // A text with invalid bytes is replaced whole, and the top still keeps its line.
    let mut invalid = changed.into_bytes();
    invalid.extend_from_slice(b"\xff\n");
    let (reloaded, from) = read_again(&editor, id, &invalid);
    editor.reload(id, reloaded, from);
    assert_eq!(editor.top_row(), 20);
}
