//! Tests of writing a buffer under a new name, without the disk.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;
use std::time::SystemTime;

use super::super::edit_tests::plain_edit;
use super::super::test_buffers::secret_buffer;
use super::*;

fn stamp(len: u64) -> FileStamp {
    FileStamp {
        modified: SystemTime::UNIX_EPOCH,
        len,
        mode: 0o644,
    }
}

fn job(editor: &mut Editor) -> SaveJob {
    let mut jobs = editor.take_save_jobs();
    assert_eq!(jobs.len(), 1);
    jobs.pop().unwrap()
}

fn path(name: &str) -> PathBuf {
    PathBuf::from("/notes").join(name)
}

#[test]
fn a_buffer_with_no_file_is_written_with_its_final_newline_and_then_visits_it() {
    let mut editor = Editor::default();
    let id = editor.window().buffer();
    plain_edit(&mut editor, id, 0..0, "text");
    assert_eq!(editor.start_write(path("new.md"), false), Ok(true));
    let job = job(&mut editor);
    assert_eq!(job.path, path("new.md"));
    assert_eq!(job.text.to_string(), "text\n");
    assert!(
        job.baseline.is_none(),
        "the host finds where the path leads"
    );
    assert_eq!(job.elsewhere, Some(Elsewhere { over: None }));
    assert!(editor.current().1.is_saving());
    let location = PathBuf::from("/real/new.md");
    let judge = |buffer: &mut Buffer| buffer.judge_secret(Some(std::slice::from_ref(&location)));
    editor.written_elsewhere(
        id,
        path("new.md"),
        job.content,
        location.clone(),
        stamp(5),
        judge,
    );
    let buffer = editor.current().1;
    assert_eq!(buffer.path(), Some(path("new.md").as_path()));
    assert_eq!(buffer.name(), "new.md");
    assert!(!buffer.is_modified());
    assert!(!buffer.is_saving());
    assert!(!buffer.is_secret());
    assert_eq!(
        buffer.baseline().map(|baseline| &baseline.location),
        Some(&location)
    );
}

#[test]
fn an_unmodified_buffer_is_still_written_and_a_read_only_or_saving_one_is_not() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a", "kept\n"));
    assert_eq!(editor.start_write(path("a.md"), false), Ok(false));
    assert_eq!(
        editor.start_write(path("b.md"), false),
        Err(SaveRefused::AlreadySaving)
    );
    let mut editor = Editor::with_buffer(Buffer::visiting(path("loading.md")));
    assert!(matches!(
        editor.start_write(path("b.md"), false),
        Err(SaveRefused::ReadOnly(_))
    ));
}

#[test]
fn the_buffers_own_path_is_a_save() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a", "kept\n"));
    let id = editor.window().buffer();
    editor.buffer_mut(id).unwrap().set_path(path("a.md"));
    assert_eq!(
        editor.start_write(path("a.md"), false),
        Err(SaveRefused::Unmodified)
    );
}

#[test]
fn a_file_found_there_is_asked_about_and_written_over_only_for_that_path() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a", "kept\n"));
    let id = editor.window().buffer();
    editor.may_write_over(id, path("there.md"), stamp(9));
    assert_eq!(editor.write_over_asked(), Some(path("there.md").as_path()));
    assert_eq!(
        editor.start_write(path("there.md"), false),
        Err(SaveRefused::Exists),
        "not agreed yet"
    );
    assert_eq!(editor.start_write(path("there.md"), true), Ok(false));
    assert_eq!(
        job(&mut editor).elsewhere,
        Some(Elsewhere {
            over: Some(stamp(9))
        })
    );
    assert_eq!(editor.write_over_asked(), None, "the answer is spent");
}

#[test]
fn agreeing_about_another_path_writes_over_nothing() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a", "kept\n"));
    let id = editor.window().buffer();
    editor.may_write_over(id, path("there.md"), stamp(9));
    assert_eq!(editor.start_write(path("elsewhere.md"), true), Ok(false));
    assert_eq!(job(&mut editor).elsewhere, Some(Elsewhere { over: None }));
}

#[test]
fn a_secret_buffer_stays_secret_under_an_ordinary_name() {
    let mut editor = Editor::with_buffer(secret_buffer());
    let id = editor.window().buffer();
    assert!(editor.current().1.is_secret());
    let location = PathBuf::from("/notes/plain.md");
    let judge = |buffer: &mut Buffer| buffer.judge_secret(Some(std::slice::from_ref(&location)));
    editor.written_elsewhere(
        id,
        location.clone(),
        editor.current().1.content_id(),
        location.clone(),
        stamp(0),
        judge,
    );
    assert!(editor.current().1.is_secret());
}

#[test]
fn an_ordinary_buffer_written_to_a_secret_name_becomes_secret() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a", "kept\n"));
    let id = editor.window().buffer();
    assert!(!editor.current().1.is_secret());
    let location = PathBuf::from("/notes/.env");
    let judge = |buffer: &mut Buffer| buffer.judge_secret(Some(std::slice::from_ref(&location)));
    let content = editor.current().1.content_id();
    editor.written_elsewhere(
        id,
        location.clone(),
        content,
        location.clone(),
        stamp(5),
        judge,
    );
    assert!(editor.current().1.is_secret());
}

#[test]
fn moving_to_a_new_name_changes_the_naming_and_tells_files_apart() {
    let mut editor = Editor::default();
    let other = editor.add_buffer(Buffer::visiting(PathBuf::from("/home/notes.md")));
    let id = editor.window().buffer();
    let naming = editor.current().1.naming();
    let location = path("notes.md");
    let content = editor.current().1.content_id();
    let judge = |buffer: &mut Buffer| buffer.judge_secret(Some(std::slice::from_ref(&location)));
    editor.written_elsewhere(
        id,
        location.clone(),
        content,
        location.clone(),
        stamp(0),
        judge,
    );
    assert_ne!(editor.current().1.naming(), naming);
    assert_eq!(editor.current().1.name(), "notes.md<notes>");
    assert_eq!(editor.buffer(other).unwrap().name(), "notes.md<home>");
}
