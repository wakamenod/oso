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
    assert!(
        editor
            .trim_reported(report)
            .is_some_and(|went| went.refused.is_none())
    );
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
    assert!(
        editor
            .trim_reported(report)
            .is_some_and(|went| went.refused.is_none())
    );
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

#[test]
fn an_agents_save_that_waits_for_a_scan_is_named_for_its_reply() {
    let mut editor = visiting("notes.txt", "");
    put_cursor(&mut editor, 0);
    edit(&mut editor, 0..0, &large());
    let id = editor.window().buffer();
    editor.for_agent(save).unwrap();
    assert_eq!(editor.take_agent_trim_saves(), [id]);
    assert!(editor.take_agent_trim_saves().is_empty(), "once");
    let report = editor.take_trim_jobs().pop().unwrap().run();
    let went = editor.trim_reported(report).unwrap();
    assert!(went.for_agent && went.refused.is_none());
    assert_eq!(editor.take_save_jobs().len(), 1);
}

#[test]
fn a_save_that_can_no_longer_go_after_the_scan_says_why() {
    let mut editor = visiting("notes.txt", "");
    put_cursor(&mut editor, 0);
    edit(&mut editor, 0..0, &large());
    save(&mut editor).unwrap();
    assert!(editor.take_agent_trim_saves().is_empty(), "the person's");
    let report = editor.take_trim_jobs().pop().unwrap().run();
    // Meanwhile a look found the file changed on disk, which needs the person's agreement.
    let id = editor.window().buffer();
    editor.file_changed_on_disk(id);
    editor.may_save_over(id, stamp());
    let went = editor.trim_reported(report).unwrap();
    assert_eq!(went.refused, Some(SaveRefused::ChangedOnDisk));
    assert!(editor.take_save_jobs().is_empty());
    assert!(!editor.current().1.is_saving());
}

#[test]
fn whitespace_a_cursor_kept_is_cleaned_by_a_later_save() {
    let mut editor = visiting("notes.txt", "x\ny\n");
    edit(&mut editor, 1..1, " foo  ");
    put_cursor(&mut editor, 7);
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "x foo  \ny\n");
    let id = editor.window().buffer();
    editor.save_finished(id, editor.current().1.content_id(), stamp());
    put_cursor(&mut editor, 9);
    edit(&mut editor, 9..9, "z");
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "x foo\nyz\n");
}

#[test]
fn the_final_newline_marks_no_line_as_edited() {
    let mut editor = visiting("notes.txt", "a\nlast   ");
    put_cursor(&mut editor, 0);
    let id = editor.window().buffer();
    for _ in 0..2 {
        edit(&mut editor, 0..0, "b");
        save(&mut editor).unwrap();
        editor.save_finished(id, editor.current().1.content_id(), stamp());
    }
    assert_eq!(text(&editor), "bba\nlast   \n");
}

#[test]
fn a_write_follows_the_markdown_rule_of_its_new_name() {
    for (from, to, expected) in [
        ("notes.txt", "notes.md", "a  \nb\n"),
        ("notes.md", "notes.txt", "a\nb\n"),
    ] {
        let mut editor = visiting(from, "x\n");
        edit(&mut editor, 0..2, "a  \nb ");
        put_cursor(&mut editor, 0);
        editor.begin_undo_group();
        editor
            .start_write(PathBuf::from("/nowhere").join(to), false)
            .unwrap();
        editor.end_undo_group();
        assert_eq!(text(&editor), expected, "{from} as {to}");
    }
}

#[test]
fn a_failed_write_leaves_the_lines_edited_for_the_next_save() {
    let mut editor = visiting("notes.txt", "a\n");
    editor.set_trim_whitespace(false);
    put_cursor(&mut editor, 0);
    edit(&mut editor, 1..1, "  ");
    save(&mut editor).unwrap();
    let id = editor.window().buffer();
    editor.take_save_jobs();
    editor.save_failed(id);
    editor.set_trim_whitespace(true);
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "a\n");
}

#[test]
fn many_ranges_with_no_text_are_scanned_on_a_thread() {
    let mut editor = visiting("notes.txt", &"x\n".repeat(5000));
    put_cursor(&mut editor, 0);
    let id = editor.window().buffer();
    // 5000 deletions leave 5000 empty ranges.
    let deletions: Vec<Replacement<'_>> = (0..5000)
        .map(|line| Replacement {
            range: line * 2..line * 2 + 1,
            text: "",
        })
        .collect();
    editor.begin_undo_group();
    editor.apply_edits(id, &deletions).unwrap();
    editor.end_undo_group();
    assert_eq!(save(&mut editor), Ok(false));
    assert!(editor.trim_waits(id));
}

#[test]
fn a_long_run_of_blank_lines_at_the_end_is_read_on_a_thread() {
    let mut editor = visiting(
        "notes.txt",
        &format!("a{}", "\n".repeat(TRIM_ON_THE_UI_THREAD)),
    );
    put_cursor(&mut editor, 0);
    let end = editor.current().1.text().len();
    edit(&mut editor, end..end, " ");
    assert_eq!(save(&mut editor), Ok(false), "it waits");
    let report = editor.take_trim_jobs().pop().unwrap().run();
    assert!(editor.trim_reported(report).is_some());
    assert_eq!(text(&editor), "a\n");
}

#[test]
fn a_cursor_in_the_blank_lines_at_the_end_keeps_what_is_before_it() {
    let mut editor = visiting("notes.txt", "a\n");
    edit(&mut editor, 2..2, "\n    ");
    put_cursor(&mut editor, 7);
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "a\n    \n");
}

#[test]
fn a_command_and_a_save_waiting_on_one_buffer_each_get_their_own_report() {
    let pasted = large();
    let mut editor = visiting("notes.txt", "");
    put_cursor(&mut editor, 0);
    edit(&mut editor, 0..0, &pasted);
    save(&mut editor).unwrap();
    let mut jobs = editor.take_trim_jobs();
    // The command over the whole buffer waits too.
    assert_eq!(editor.delete_trailing_whitespace(), Ok(false));
    jobs.extend(editor.take_trim_jobs());
    assert_eq!(jobs.len(), 2);
    let command = jobs.pop().unwrap().run();
    let save_scan = jobs.pop().unwrap().run();
    // The command's report goes to the command, which saves nothing.
    editor.trim_reported(command).unwrap();
    assert!(editor.take_save_jobs().is_empty());
    // The save's own scan is stale now, so it scans again, and then saves.
    assert_eq!(editor.trim_reported(save_scan), None);
    let rescan = editor.take_trim_jobs().pop().unwrap().run();
    editor.trim_reported(rescan).unwrap();
    assert_eq!(editor.take_save_jobs().len(), 1);
    assert_eq!(text(&editor), pasted.replace(" \n", "\n"));
}

#[test]
fn a_command_waiting_for_its_scan_follows_the_edits_made_meanwhile() {
    let mut editor = visiting("notes.txt", &large());
    put_cursor(&mut editor, 0);
    assert_eq!(editor.delete_trailing_whitespace(), Ok(false));
    let stale = editor.take_trim_jobs().pop().unwrap().run();
    edit(&mut editor, 0..0, "new  \n");
    assert_eq!(editor.trim_reported(stale), None);
    let report = editor.take_trim_jobs().pop().unwrap().run();
    editor.trim_reported(report).unwrap();
    let cleaned = text(&editor);
    assert!(
        cleaned.starts_with("new\nline\nline\n"),
        "{}",
        &cleaned[..20]
    );
    assert!(!cleaned.contains(" \n"));
}

#[test]
fn a_buffer_with_no_file_written_as_markdown_follows_its_rule() {
    let mut editor = Editor::with_buffer(Buffer::new("*scratch*"));
    editor.set_view_size(ViewSize::new(5, 20));
    editor.set_trim_whitespace(true);
    edit(&mut editor, 0..0, "a  \nb ");
    put_cursor(&mut editor, 0);
    editor.begin_undo_group();
    editor
        .start_write(PathBuf::from("/nowhere/x.md"), false)
        .unwrap();
    editor.end_undo_group();
    assert_eq!(text(&editor), "a  \nb\n");
}

#[test]
fn a_write_under_a_new_name_that_lands_forgets_the_lines_it_took() {
    let mut editor = visiting("notes.txt", "a\nb\n");
    editor.set_trim_whitespace(false);
    edit(&mut editor, 1..1, "  ");
    put_cursor(&mut editor, 0);
    let to = PathBuf::from("/nowhere/other.txt");
    editor.begin_undo_group();
    editor.start_write(to.clone(), false).unwrap();
    editor.end_undo_group();
    let job = editor.take_save_jobs().pop().unwrap();
    let id = editor.window().buffer();
    editor.written_elsewhere(id, to.clone(), job.content, to, stamp(), |_| {});
    // A later save cleans only what was edited after the write.
    editor.set_trim_whitespace(true);
    edit(&mut editor, 5..5, "c ");
    save(&mut editor).unwrap();
    assert_eq!(text(&editor), "a  \nbc\n");
}
