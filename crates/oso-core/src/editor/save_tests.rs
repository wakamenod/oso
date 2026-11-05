//! The tests of what a save and an exit need from the editor state. Commands and
//! their keys are tested through the dispatcher, in the tip crate.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::{Editor, ExitAction, ExitRefused, SaveJob, SaveRefused};
use crate::buffer::{Baseline, Buffer, FileStamp, ReadOnly, Replacement};
use crate::decode::LineEnding;
use crate::display_map::TabWidth;
use crate::load::decode_all;
use crate::selection::{Selection, SelectionSet};
use crate::window::{Scroll, ViewSize};

fn path() -> PathBuf {
    PathBuf::from("/nowhere/notes.txt")
}

fn stamp(len: u64) -> FileStamp {
    FileStamp {
        modified: SystemTime::UNIX_EPOCH + Duration::from_secs(5),
        len,
        mode: 0o644,
    }
}

fn visiting(content: &[u8]) -> Editor {
    let mut buffer = Buffer::visiting(path());
    buffer.apply_load(decode_all(content, TabWidth::default()));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(5, 20));
    editor
}

fn insert(editor: &mut Editor, offset: usize, text: &str) {
    let id = editor.window().buffer();
    let edit = Replacement {
        range: offset..offset,
        text,
    };
    editor.apply_edits(id, &[edit]).unwrap();
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn job(editor: &mut Editor) -> SaveJob {
    let mut jobs = editor.take_save_jobs();
    assert_eq!(jobs.len(), 1);
    jobs.pop().unwrap()
}

fn cursors(editor: &Editor) -> Vec<usize> {
    let selections = &editor.window().selections;
    selections
        .iter()
        .map(|selection| selection.cursor)
        .collect()
}

fn place(editor: &mut Editor, at: &[usize]) {
    let selections = at.iter().map(|cursor| Selection::at(*cursor)).collect();
    let set = SelectionSet::from_parts(selections, 0).unwrap();
    editor.restore_positions(set, Scroll::default());
}

#[test]
fn a_job_is_a_snapshot_of_the_text_with_what_the_write_needs() {
    let mut editor = visiting(b"\xEF\xBB\xBFabc\n");
    let baseline = Baseline {
        location: PathBuf::from("/real/notes.txt"),
        file: Some(stamp(7)),
    };
    let id = editor.window().buffer();
    editor
        .buffer_mut(id)
        .unwrap()
        .set_baseline(baseline.clone());
    insert(&mut editor, 0, "x");
    assert_eq!(editor.start_save(), Ok(false));
    let job = job(&mut editor);
    assert_eq!(job.buffer, id);
    assert_eq!(job.path, path());
    assert_eq!(job.text.to_string(), "xabc\n");
    assert!(job.bom);
    assert_eq!(job.content, editor.current().1.content_id());
    assert_eq!(job.baseline, Some(baseline));

    insert(&mut editor, 0, "later");
    assert_eq!(
        job.text.to_string(),
        "xabc\n",
        "later edits do not reach it"
    );
    assert!(editor.take_save_jobs().is_empty(), "taken once");
}

#[test]
fn a_stamp_tells_content_apart_by_time_and_size_and_not_by_mode() {
    let chmod = FileStamp {
        mode: 0o600,
        ..stamp(5)
    };
    assert!(stamp(5).same_content(chmod));
    assert!(!stamp(5).same_content(stamp(6)));
    let touched = FileStamp {
        modified: SystemTime::UNIX_EPOCH,
        ..stamp(5)
    };
    assert!(!stamp(5).same_content(touched));
}

#[test]
fn refusals_say_why_and_change_nothing() {
    let refused = |mut editor: Editor| {
        let before = text(&editor);
        let refusal = editor.start_save().unwrap_err();
        assert_eq!(text(&editor), before);
        assert!(editor.take_save_jobs().is_empty());
        assert!(!editor.current().1.is_saving());
        refusal
    };
    assert_eq!(
        refused(visiting(b"a\0")),
        SaveRefused::ReadOnly(ReadOnly::Binary)
    );
    assert_eq!(
        refused(visiting(b"a\xff")),
        SaveRefused::ReadOnly(ReadOnly::InvalidUtf8)
    );
    assert_eq!(
        refused(Editor::with_buffer(Buffer::visiting(path()))),
        SaveRefused::ReadOnly(ReadOnly::Loading)
    );
    assert_eq!(refused(visiting(b"abc")), SaveRefused::Unmodified);
    assert_eq!(refused(Editor::default()), SaveRefused::Unmodified);
    let mut scratch = Editor::default();
    insert(&mut scratch, 0, "x");
    assert_eq!(refused(scratch), SaveRefused::NoFile);
}

#[test]
fn every_refusal_reads_as_a_sentence_of_its_own() {
    let messages = [
        SaveRefused::ReadOnly(ReadOnly::Binary),
        SaveRefused::ReadOnly(ReadOnly::InvalidUtf8),
        SaveRefused::ReadOnly(ReadOnly::Loading),
        SaveRefused::AlreadySaving,
        SaveRefused::Unmodified,
        SaveRefused::NoFile,
    ]
    .map(|refused| refused.to_string());
    for (index, message) in messages.iter().enumerate() {
        assert!(!message.is_empty());
        assert!(!messages[..index].contains(message), "{message}");
    }
    assert!(messages[0].contains("binary"));
    assert!(messages[1].contains("UTF-8"));
    assert!(messages[2].contains("loading"));
    assert!(messages[3].contains("already"));
}

#[test]
fn a_second_save_is_refused_while_one_runs_and_starts_after_it_ends() {
    let mut editor = visiting(b"abc\n");
    insert(&mut editor, 0, "x");
    editor.start_save().unwrap();
    assert!(editor.current().1.is_saving());
    let first = job(&mut editor);
    insert(&mut editor, 0, "y");
    assert_eq!(editor.start_save(), Err(SaveRefused::AlreadySaving));
    assert!(editor.take_save_jobs().is_empty());

    let id = first.buffer;
    editor.save_finished(id, first.content, stamp(5));
    assert!(!editor.current().1.is_saving());
    assert!(editor.current().1.is_modified(), "the edit is not on disk");
    assert_eq!(editor.start_save(), Ok(false));
    assert_eq!(job(&mut editor).text.to_string(), "yxabc\n");
}

#[test]
fn a_finished_save_records_the_state_and_the_baseline_and_a_failed_one_neither() {
    let mut editor = visiting(b"abc\n");
    let id = editor.window().buffer();
    let location = PathBuf::from("/real/notes.txt");
    let baseline = Baseline {
        location: location.clone(),
        file: Some(stamp(4)),
    };
    editor
        .buffer_mut(id)
        .unwrap()
        .set_baseline(baseline.clone());
    insert(&mut editor, 0, "x");
    editor.start_save().unwrap();
    let job = job(&mut editor);
    editor.save_failed(id);
    assert!(!editor.current().1.is_saving());
    assert!(editor.current().1.is_modified());
    assert_eq!(editor.current().1.baseline(), Some(&baseline));

    editor.save_finished(id, job.content, stamp(5));
    assert!(!editor.current().1.is_modified());
    let written = Baseline {
        location,
        file: Some(stamp(5)),
    };
    assert_eq!(editor.current().1.baseline(), Some(&written));
}

#[test]
fn a_report_about_a_buffer_that_went_changes_nothing() {
    let mut editor = visiting(b"abc\n");
    let id = editor.window().buffer();
    insert(&mut editor, 0, "x");
    editor.start_save().unwrap();
    let job = job(&mut editor);
    editor.kill_buffer(id);
    editor.save_finished(id, job.content, stamp(5));
    editor.save_failed(id);
    assert_eq!(editor.current().1.name(), crate::editor::SCRATCH);
}

#[test]
fn the_final_newline_uses_the_files_line_ending_and_only_when_needed() {
    let saved_text = |content: &[u8]| {
        let mut editor = visiting(content);
        // A change at the start, which leaves the end as it is.
        insert(&mut editor, 0, "x");
        editor.start_save().unwrap();
        job(&mut editor).text.to_string()
    };
    assert_eq!(saved_text(b"a\nb"), "xa\nb\n");
    assert_eq!(saved_text(b"a\r\nb"), "xa\r\nb\r\n");
    assert_eq!(saved_text(b"a\n"), "xa\n");
    assert_eq!(saved_text(b"a\r\n"), "xa\r\n");
    assert_eq!(saved_text(b""), "x\n");
    assert_eq!(
        visiting(b"a\r\nb").current().1.file_info().line_ending,
        LineEnding::Crlf
    );
    assert_eq!(saved_text("日本語".as_bytes()), "x日本語\n");
    assert_eq!(saved_text("🍣".repeat(2000).as_bytes()).len(), 8002);
}

#[test]
fn an_emptied_buffer_gets_no_line_break() {
    let mut editor = visiting(b"a\n");
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..2,
        text: "",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    assert_eq!(editor.start_save(), Ok(false));
    assert_eq!(job(&mut editor).text.to_string(), "");
}

#[test]
fn cursors_at_the_end_stay_before_the_new_line_break() {
    let mut editor = visiting(b"ab\ncd");
    insert(&mut editor, 0, "x");
    place(&mut editor, &[1, 6]);
    assert_eq!(editor.start_save(), Ok(true));
    assert_eq!(text(&editor), "xab\ncd\n");
    assert_eq!(cursors(&editor), [1, 6]);
}

#[test]
fn a_line_break_after_a_lone_cr_leaves_the_cursor_at_a_cluster_start() {
    let mut editor = visiting(b"ab\r");
    insert(&mut editor, 0, "x");
    place(&mut editor, &[4]);
    assert_eq!(editor.start_save(), Ok(true));
    assert_eq!(text(&editor), "xab\r\n");
    assert_eq!(cursors(&editor), [3], "before the `\\r\\n`, not inside it");
}

#[test]
fn an_exit_with_nothing_at_risk_goes_at_once() {
    let mut editor = visiting(b"abc\n");
    assert_eq!(editor.confirm_exit(ExitAction::Command), Ok(()));
    assert_eq!(editor.confirm_exit(ExitAction::CloseWindow), Ok(()));
    assert_eq!(editor.exit_attempt(), None);
}

#[test]
fn an_exit_is_refused_once_for_each_action_and_counts_what_is_at_risk() {
    let mut editor = visiting(b"abc\n");
    insert(&mut editor, 0, "x");
    let scratch = editor.add_buffer(Buffer::new("other"));
    let first = editor.window().buffer();
    editor.show(scratch);
    insert(&mut editor, 0, "y");
    editor.show(first);
    let refused = ExitRefused {
        modified: 2,
        saving: 0,
    };
    assert_eq!(editor.confirm_exit(ExitAction::Command), Err(refused));
    assert_eq!(editor.exit_attempt(), Some(ExitAction::Command));
    assert_eq!(
        editor.confirm_exit(ExitAction::CloseWindow),
        Err(refused),
        "another action does not confirm"
    );
    assert_eq!(editor.confirm_exit(ExitAction::CloseWindow), Ok(()));

    editor.reset_exit_guard();
    assert_eq!(editor.confirm_exit(ExitAction::CloseWindow), Err(refused));
}

#[test]
fn a_running_save_counts_even_when_undo_left_the_buffer_unmodified() {
    let mut editor = visiting(b"abc\n");
    insert(&mut editor, 0, "x");
    editor.start_save().unwrap();
    let job = job(&mut editor);
    editor.undo().unwrap();
    assert!(!editor.current().1.is_modified());
    assert_eq!(
        editor.confirm_exit(ExitAction::Command),
        Err(ExitRefused {
            modified: 0,
            saving: 1
        })
    );
    editor.save_finished(job.buffer, job.content, stamp(5));
    assert!(
        editor.current().1.is_modified(),
        "the disk now holds the edit"
    );
    assert_eq!(
        editor.confirm_exit(ExitAction::Command),
        Err(ExitRefused {
            modified: 1,
            saving: 0
        }),
        "the refusal spoke of a save, and this is unsaved text it never mentioned"
    );
    assert_eq!(editor.confirm_exit(ExitAction::Command), Ok(()));
}

#[test]
fn a_save_that_leaves_nothing_at_risk_lets_the_next_exit_go() {
    let mut editor = visiting(b"abc\n");
    insert(&mut editor, 0, "x");
    editor.start_save().unwrap();
    let job = job(&mut editor);
    assert!(editor.confirm_exit(ExitAction::Command).is_err());
    editor.save_finished(job.buffer, job.content, stamp(5));
    assert_eq!(editor.confirm_exit(ExitAction::CloseWindow), Ok(()));
}

#[test]
fn a_failed_save_takes_back_a_refusal_made_while_it_ran() {
    let mut editor = visiting(b"abc\n");
    let id = editor.window().buffer();
    insert(&mut editor, 0, "x");
    editor.start_save().unwrap();
    assert!(editor.confirm_exit(ExitAction::Command).is_err());
    editor.save_failed(id);
    assert!(editor.confirm_exit(ExitAction::Command).is_err());
    assert_eq!(editor.confirm_exit(ExitAction::Command), Ok(()));
}

#[test]
fn a_refused_exit_says_how_many() {
    let message = |modified, saving| ExitRefused { modified, saving }.to_string();
    assert_eq!(message(1, 0), "1 modified buffer; repeat to exit anyway");
    assert_eq!(message(3, 0), "3 modified buffers; repeat to exit anyway");
    assert_eq!(message(0, 1), "1 save running; repeat to exit anyway");
    assert_eq!(
        message(2, 2),
        "2 modified buffers and 2 saves running; repeat to exit anyway"
    );
}
