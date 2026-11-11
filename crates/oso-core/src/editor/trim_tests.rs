//! The tests of whitespace cleanup (ADR 0113): on save, of the lines edited since, and the
//! command over a buffer or its regions, at once or through a scan on a thread.
//!
//! Must not depend on: threads or the file system; a test runs the scan itself.

use std::path::PathBuf;

use super::{Editor, SaveRefused, TRIM_ON_THE_UI_THREAD};
use crate::buffer::{Buffer, EditError, ReadOnly, Replacement};
use crate::display_map::TabWidth;
use crate::load::decode_all;
use crate::selection::{MarkActive, Selection, SelectionSet};
use crate::window::ViewSize;

fn visiting(name: &str, content: &str) -> Editor {
    let mut buffer = Buffer::visiting(PathBuf::from("/nowhere").join(name));
    buffer.apply_load(decode_all(content.as_bytes(), TabWidth::default()));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(5, 20));
    editor.set_trim_whitespace(true);
    editor
}

/// Replaces `range` with `text`, as one command.
fn edit(editor: &mut Editor, range: std::ops::Range<usize>, text: &str) {
    let id = editor.window().buffer();
    editor.begin_undo_group();
    editor
        .apply_edits(id, &[Replacement { range, text }])
        .unwrap();
    editor.end_undo_group();
}

/// Saves, as the command does: in an undo group of its own.
fn save(editor: &mut Editor) -> Result<bool, SaveRefused> {
    editor.begin_undo_group();
    let saved = editor.start_save();
    editor.end_undo_group();
    saved
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn put_cursor(editor: &mut Editor, offset: usize) {
    editor.current_mut().0.selections = SelectionSet::new(offset);
}

#[test]
fn a_save_cleans_only_the_lines_edited_since_the_file_was_read() {
    let mut editor = visiting("notes.txt", "a  \nb  \nc  \n");
    put_cursor(&mut editor, 0);
    edit(&mut editor, 4..5, "B");
    assert_eq!(save(&mut editor), Ok(true));
    assert_eq!(text(&editor), "a  \nB\nc  \n");
    assert_eq!(editor.take_save_jobs().len(), 1);
    // The save forgot where it was edited: the next edit is all the next save sees.
    editor.save_finished(
        editor.window().buffer(),
        editor.current().1.content_id(),
        stamp(),
    );
    edit(&mut editor, 9..9, " ");
    assert_eq!(save(&mut editor), Ok(true));
    assert_eq!(text(&editor), "a  \nB\nc\n");
}

fn stamp() -> crate::buffer::FileStamp {
    crate::buffer::FileStamp {
        modified: std::time::SystemTime::UNIX_EPOCH,
        len: 0,
        mode: 0o644,
        identity: None,
    }
}

#[test]
fn the_cleanup_and_the_final_newline_undo_as_one_step() {
    let mut editor = visiting("notes.txt", "a\nb");
    put_cursor(&mut editor, 0);
    edit(&mut editor, 3..3, "  \n\n\n");
    assert_eq!(save(&mut editor), Ok(true));
    assert_eq!(text(&editor), "a\nb\n");
    editor.undo().unwrap();
    assert_eq!(text(&editor), "a\nb  \n\n\n");
}

#[test]
fn whitespace_before_a_cursor_on_its_line_stays() {
    let mut editor = visiting("notes.txt", "x\n");
    edit(&mut editor, 1..1, " foo  ");
    put_cursor(&mut editor, 6);
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "x foo \n");
}

#[test]
fn markdown_keeps_two_spaces_after_text() {
    let mut editor = visiting("README.MD", "x\n");
    put_cursor(&mut editor, 0);
    edit(&mut editor, 1..1, "  \ny ");
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "x  \ny\n");
}

#[test]
fn with_the_setting_off_only_the_final_newline_goes_in() {
    let mut editor = visiting("notes.txt", "a\nb");
    editor.set_trim_whitespace(false);
    put_cursor(&mut editor, 0);
    edit(&mut editor, 1..1, "  ");
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "a  \nb\n");
    // Turned on later, a save looks only at edits made since the last one.
    editor.set_trim_whitespace(true);
    editor.save_finished(
        editor.window().buffer(),
        editor.current().1.content_id(),
        stamp(),
    );
    edit(&mut editor, 5..5, "c");
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "a  \nbc\n");
}

#[test]
fn a_write_under_a_new_name_cleans_as_a_save_does() {
    let mut editor = visiting("notes.txt", "a\n");
    put_cursor(&mut editor, 0);
    edit(&mut editor, 1..1, " \t");
    editor.begin_undo_group();
    assert_eq!(
        editor.start_write(PathBuf::from("/nowhere/other.txt"), false),
        Ok(true)
    );
    editor.end_undo_group();
    assert_eq!(text(&editor), "a\n");
    assert_eq!(editor.take_save_jobs().len(), 1);
}

/// A paste past the budget, with trailing whitespace on its lines.
fn large() -> String {
    "line \n".repeat(TRIM_ON_THE_UI_THREAD / 6 + 1)
}

#[test]
fn past_the_budget_the_save_waits_for_a_scan_on_a_thread() {
    let mut editor = visiting("notes.txt", "");
    put_cursor(&mut editor, 0);
    let pasted = large();
    edit(&mut editor, 0..0, &pasted);
    assert_eq!(save(&mut editor), Ok(false));
    assert!(editor.take_save_jobs().is_empty());
    let id = editor.window().buffer();
    assert!(editor.trim_waits(id));
    // A second save is refused while it waits.
    assert_eq!(save(&mut editor), Err(SaveRefused::AlreadySaving));
    let mut jobs = editor.take_trim_jobs();
    assert_eq!(jobs.len(), 1);
    let report = jobs.pop().unwrap().run();
    assert_eq!(editor.trim_reported(report), Some(false));
    assert_eq!(text(&editor), pasted.replace(" \n", "\n"));
    assert!(!editor.trim_waits(id));
    assert_eq!(editor.take_save_jobs().len(), 1);
    // The cleanup is a step of its own, and the paste one more.
    editor.undo().unwrap();
    assert_eq!(text(&editor), pasted);
}

#[test]
fn an_edit_during_the_scan_scans_again() {
    let mut editor = visiting("notes.txt", "");
    put_cursor(&mut editor, 0);
    let pasted = large();
    edit(&mut editor, 0..0, &pasted);
    save(&mut editor).unwrap();
    let report = editor.take_trim_jobs().pop().unwrap().run();
    edit(&mut editor, 0..0, "new ");
    assert_eq!(editor.trim_reported(report), None);
    let report = editor.take_trim_jobs().pop().unwrap().run();
    assert_eq!(editor.trim_reported(report), Some(false));
    assert!(text(&editor).starts_with("new line\nline\n"));
    assert_eq!(editor.take_save_jobs().len(), 1);
}

#[test]
fn the_command_cleans_the_whole_buffer_or_its_regions() {
    let mut editor = visiting("notes.txt", "a  \nb  \nc  \n\n\n");
    editor.set_trim_whitespace(false);
    assert_eq!(editor.delete_trailing_whitespace(), Ok(true));
    assert_eq!(text(&editor), "a\nb\nc\n");

    let mut editor = visiting("notes.txt", "a  \nb  \nc  \n");
    let window = editor.current_mut().0;
    window.selections = SelectionSet::from_parts(
        vec![Selection {
            cursor: 5,
            mark: Some(4),
            goal: None,
        }],
        0,
    )
    .unwrap();
    window.selections.set_mark_active(MarkActive::Active);
    assert_eq!(editor.delete_trailing_whitespace(), Ok(true));
    assert_eq!(text(&editor), "a  \nb\nc  \n");
}

#[test]
fn the_command_refuses_a_read_only_buffer() {
    let mut buffer = Buffer::visiting(PathBuf::from("/nowhere/notes.txt"));
    let mut snapshot = decode_all(b"a  \n", TabWidth::default());
    snapshot.done = false;
    buffer.apply_load(snapshot);
    let mut editor = Editor::with_buffer(buffer);
    assert_eq!(
        editor.delete_trailing_whitespace(),
        Err(EditError::ReadOnly(ReadOnly::Loading))
    );
}
