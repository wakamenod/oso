//! The tests of safe save, on a scratch directory with no window and, but for one
//! test, no thread.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use crate::commands::kept_messages;
use oso_core::buffer::{Baseline, Buffer, Replacement};
use oso_core::display_map::TabWidth;
use oso_core::editor::{Editor, SaveJob};
use oso_core::load::decode_all;

use crate::loader::{location, stamp_of};

use super::{
    PRIVATE_MODE, SaveError, SaveReport, Temporary, Unsynced, apply, check_whole, prepare,
    prepare_with, run, run_with, spawn, unless_unsupported, write_text,
};

/// A scratch directory, with no links in its path, removed at the end of the test.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new(test: &str) -> Self {
        let name = format!("oso-saver-test-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    /// The names in the directory, sorted, which any temporary file left would be in.
    pub(crate) fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The baseline of the file at `path`, as the loader takes it.
fn baseline(path: &Path) -> Baseline {
    Baseline {
        location: location(path).unwrap(),
        file: fs::metadata(path)
            .ok()
            .map(|metadata| stamp_of(&metadata).unwrap()),
    }
}

/// A buffer that has loaded `path`, or found it missing, as the app does.
pub(crate) fn visited(path: &Path) -> Buffer {
    let mut buffer = Buffer::visiting(path.to_path_buf());
    match fs::read(path) {
        Ok(content) => buffer.apply_load(decode_all(&content, TabWidth::default())),
        Err(_) => buffer.finish_empty(),
    }
    if location(path).is_ok() {
        buffer.set_baseline(baseline(path));
    }
    buffer.judge_secret(Some(&[path.to_path_buf()]));
    buffer
}

/// An editor showing `path`, as [`visited`].
pub(crate) fn visit(path: &Path) -> Editor {
    Editor::with_buffer(visited(path))
}

/// A job writing `text` over what `editor` shows.
fn job_of(editor: &mut Editor, text: &str) -> SaveJob {
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..usize::MAX,
        text,
    };
    editor.apply_edits(id, &[edit]).unwrap();
    editor.start_save().unwrap();
    editor.take_save_jobs().pop().unwrap()
}

/// A job writing `text` over the file at `path`, read just now.
fn job(path: &Path, text: &str) -> SaveJob {
    job_of(&mut visit(path), text)
}

/// Sets a file's modification time an hour on, with the same size and content.
fn touch(path: &Path) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    file.set_modified(modified + Duration::from_secs(3600))
        .unwrap();
}

#[track_caller]
fn assert_written(report: &SaveReport) {
    assert!(report.outcome.is_ok(), "{:?}", report.outcome);
}

#[track_caller]
fn assert_conflict(report: &SaveReport) {
    assert!(
        matches!(report.outcome, Err(SaveError::Conflict)),
        "{:?}",
        report.outcome
    );
}

#[test]
fn a_file_changed_under_edits_warns_until_a_save_gets_through() {
    let scratch = Scratch::new("changed-warning");
    let path = scratch.file("notes.txt", b"one\n");
    let mut editor = visit(&path);
    let job = job_of(&mut editor, "two\n");
    fs::write(&path, b"another writer, and longer\n").unwrap();
    let report = run(job);
    assert_conflict(&report);
    apply(&mut editor, report);
    assert_eq!(warnings(&editor), "[changed on disk] ");

    // The disk holds what the buffer last read again, so a save can go through.
    fs::write(&path, b"one\n").unwrap();
    let id = editor.window().buffer();
    let baseline = baseline(&path);
    editor.buffer_mut(id).unwrap().set_baseline(baseline);
    editor.start_save().unwrap();
    let job = editor.take_save_jobs().pop().unwrap();
    let report = run(job);
    assert_written(&report);
    apply(&mut editor, report);

    // Unsaved edits again, and no warning: nothing has changed under them since.
    let edit = Replacement {
        range: 0..0,
        text: "x",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    assert_eq!(warnings(&editor), "");
}

/// What the modeline warns of.
fn warnings(editor: &Editor) -> String {
    let mut status = oso_core::editor::Status::default();
    editor.status_into(&mut status);
    status.warnings
}

#[test]
fn the_text_replaces_the_file_and_nothing_is_left_beside_it() {
    let scratch = Scratch::new("round-trip");
    let path = scratch.file("notes.txt", b"old\n");
    let text = "日本語 🍣\n".repeat(30_000);
    assert_written(&run(job(&path, &text)));
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(scratch.names(), ["notes.txt"]);
}

#[test]
fn the_bom_stays_and_none_is_added() {
    let scratch = Scratch::new("bom");
    let with = scratch.file("with.txt", b"\xEF\xBB\xBFold\n");
    assert_written(&run(job(&with, "new\n")));
    assert_eq!(fs::read(&with).unwrap(), b"\xEF\xBB\xBFnew\n");
    let without = scratch.file("without.txt", b"old\n");
    assert_written(&run(job(&without, "new\n")));
    assert_eq!(fs::read(&without).unwrap(), b"new\n");
}

#[test]
fn crlf_stays_as_it_is_in_the_text() {
    let scratch = Scratch::new("crlf");
    let path = scratch.file("dos.txt", b"one\r\ntwo\r\n");
    let mut editor = visit(&path);
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..0,
        text: "x",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    editor.start_save().unwrap();
    assert_written(&run(editor.take_save_jobs().pop().unwrap()));
    assert_eq!(fs::read(&path).unwrap(), b"xone\r\ntwo\r\n");
}

#[test]
fn saving_twice_finds_no_conflict_with_itself() {
    let scratch = Scratch::new("twice");
    let path = scratch.file("notes.txt", b"old\n");
    let mut editor = visit(&path);
    for text in ["first\n", "second\n"] {
        let job = job_of(&mut editor, text);
        apply(&mut editor, run(job));
        assert!(!editor.current().1.is_modified());
        assert_eq!(editor.current().1.baseline(), Some(&baseline(&path)));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
}

#[test]
fn a_new_file_is_created_with_the_default_mode() {
    let scratch = Scratch::new("new");
    let path = scratch.path().join("new.txt");
    assert_written(&run(job(&path, "new\n")));
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
    assert_eq!(scratch.names(), ["new.txt"]);
    #[cfg(unix)]
    {
        let control = scratch.file("control.txt", b"");
        assert_eq!(mode(&path), mode(&control), "what the umask gives");
    }
}

#[test]
fn a_changed_file_is_kept_and_the_write_never_starts() {
    let scratch = Scratch::new("changed");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    fs::write(&path, b"other program\n").unwrap();
    assert_conflict(&run(job));
    assert_eq!(fs::read(&path).unwrap(), b"other program\n");
    assert_eq!(scratch.names(), ["notes.txt"]);
}

#[test]
fn a_change_with_the_same_size_counts_by_its_time_even_going_backward() {
    let scratch = Scratch::new("same-size");
    let path = scratch.file("notes.txt", b"old\n");
    let job_after = job(&path, "new\n");
    touch(&path);
    assert_conflict(&run(job_after));

    let older = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let job_before = job(&path, "new\n");
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_modified(older).unwrap();
    assert_conflict(&run(job_before));
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
}

#[test]
fn a_change_during_the_write_is_kept_and_the_temporary_file_goes() {
    let scratch = Scratch::new("late-change");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    let prepared = prepare(&job).unwrap();
    assert_eq!(scratch.names().len(), 2, "the temporary file is beside it");
    fs::write(&path, b"other program\n").unwrap();
    assert!(matches!(prepared.commit(&job), Err(SaveError::Conflict)));
    assert_eq!(fs::read(&path).unwrap(), b"other program\n");
    assert_eq!(scratch.names(), ["notes.txt"]);
}

#[test]
fn a_file_that_appeared_is_kept() {
    let scratch = Scratch::new("appeared");
    let path = scratch.path().join("new.txt");
    let early = job(&path, "mine\n");
    let late = job(&path, "mine\n");
    let prepared = prepare(&late).unwrap();
    fs::write(&path, b"theirs\n").unwrap();
    assert_conflict(&run(early));
    assert!(matches!(prepared.commit(&late), Err(SaveError::Conflict)));
    assert_eq!(fs::read(&path).unwrap(), b"theirs\n");
    assert_eq!(scratch.names(), ["new.txt"]);
}

#[test]
fn a_deleted_file_holds_nothing_to_keep_and_is_written_again() {
    let scratch = Scratch::new("deleted");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    fs::remove_file(&path).unwrap();
    assert_written(&run(job));
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
}

#[test]
fn a_directory_in_the_files_place_is_a_conflict() {
    let scratch = Scratch::new("directory");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_conflict(&run(job));
    assert!(path.is_dir());
    assert_eq!(scratch.names(), ["notes.txt"]);
}

#[test]
fn a_missing_directory_is_an_error_and_is_not_created() {
    let scratch = Scratch::new("no-directory");
    let directory = scratch.path().join("no-such-directory");
    let path = directory.join("new.txt");
    let report = run(job(&path, "new\n"));
    assert!(
        matches!(report.outcome, Err(SaveError::Io(_))),
        "{report:?}"
    );
    assert!(!directory.exists());
    assert!(scratch.names().is_empty());
}

#[test]
fn a_file_name_as_long_as_the_system_allows_still_saves() {
    let scratch = Scratch::new("long-name");
    let name = "n".repeat(255);
    let path = scratch.file(&name, b"old\n");
    assert_written(&run(job(&path, "new\n")));
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
    assert_eq!(scratch.names(), [name]);
}

#[test]
fn the_temporary_file_goes_when_a_panic_passes_through() {
    let scratch = Scratch::new("panic");
    let directory = scratch.path().to_path_buf();
    let panicked = std::panic::catch_unwind(move || {
        let _kept = Temporary::create(&directory, PRIVATE_MODE).unwrap();
        panic!("a bug in the middle of a write");
    });
    assert!(panicked.is_err());
    assert!(scratch.names().is_empty());
}

/// A writer that takes some bytes and then fails, as a full disk does.
struct FullDisk(usize);

impl std::io::Write for FullDisk {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0 == 0 {
            return Err(std::io::Error::other("no space left"));
        }
        let taken = bytes.len().min(self.0);
        self.0 -= taken;
        Ok(taken)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_failing_write_is_an_error() {
    let scratch = Scratch::new("full");
    let path = scratch.path().join("new.txt");
    let job = job(&path, "new\n");
    assert!(write_text(FullDisk(1), &job).is_err());
    assert!(write_text(FullDisk(100), &job).is_ok());
}

#[test]
fn an_unmodified_buffer_writes_only_if_its_file_has_gone() {
    let scratch = Scratch::new("gone");
    let path = scratch.file("notes.txt", b"held only here");
    let mut editor = visit(&path);
    let there = {
        editor.start_save().unwrap();
        editor.take_save_jobs().pop().unwrap()
    };
    assert!(there.only_if_gone);
    let report = run(there);
    assert!(matches!(report.outcome, Ok(None)), "{report:?}");
    assert_eq!(
        applied(&mut editor, report),
        "(No changes need to be saved)"
    );
    assert_eq!(fs::read(&path).unwrap(), b"held only here");

    // Changed on disk is no business of an unmodified buffer's until it reloads.
    fs::write(&path, b"another program").unwrap();
    editor.start_save().unwrap();
    let report = run(editor.take_save_jobs().pop().unwrap());
    assert!(matches!(report.outcome, Ok(None)), "{report:?}");
    apply(&mut editor, report);
    assert_eq!(fs::read(&path).unwrap(), b"another program");

    fs::remove_file(&path).unwrap();
    editor.start_save().unwrap();
    let report = run(editor.take_save_jobs().pop().unwrap());
    let message = applied(&mut editor, report);
    assert!(message.starts_with("Wrote"), "{message}");
    assert_eq!(
        fs::read(&path).unwrap(),
        b"held only here",
        "as it was, with no final newline added to an unmodified buffer"
    );
    assert!(!editor.current().1.is_modified());
}

#[test]
fn a_file_shorter_than_the_text_is_refused_before_it_replaces_anything() {
    let scratch = Scratch::new("short");
    let with_bom = scratch.file("with.txt", b"\xEF\xBB\xBFold\n");
    let job = job(&with_bom, "new\n");
    assert!(check_whole(7, &job).is_ok(), "the BOM and the text");
    assert!(check_whole(4, &job).is_err(), "without the BOM");
    assert!(check_whole(6, &job).is_err(), "cut short");
    assert!(check_whole(8, &job).is_err(), "more than was written");
}

#[test]
fn a_file_cut_short_while_it_was_written_never_replaces_anything() {
    let scratch = Scratch::new("cut-short");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new text\n");
    let cut = prepare_with(&job, |file| file.set_len(1));
    assert!(matches!(cut, Err(SaveError::Io(_))), "not refused");
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
    assert_eq!(scratch.names(), ["notes.txt"]);
}

#[test]
fn a_sync_the_file_system_cannot_do_still_saves_and_says_so_through_prepare() {
    let scratch = Scratch::new("no-sync");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    let prepared = prepare_with(&job, |_| Err(std::io::ErrorKind::Unsupported.into()));
    let Ok(prepared) = prepared else {
        panic!("an unsupported sync failed the save");
    };
    let written = prepared.commit(&job).map_err(|_| "no commit").unwrap();
    assert!(matches!(written.unsynced, Some(Unsynced::File(_))));
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
}

#[test]
fn a_sync_that_really_failed_fails_the_save_through_prepare() {
    let scratch = Scratch::new("bad-sync");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    let failed = prepare_with(&job, |_| Err(std::io::Error::other("the disk failed")));
    assert!(matches!(failed, Err(SaveError::Io(_))), "not refused");
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
    assert_eq!(scratch.names(), ["notes.txt"]);
}

#[test]
fn a_change_right_after_a_save_counts_as_a_change_at_the_next() {
    let scratch = Scratch::new("after-rename");
    let path = scratch.file("notes.txt", b"old\n");
    let mut editor = visit(&path);
    let first = job_of(&mut editor, "first\n");
    apply(&mut editor, run(first));
    fs::write(&path, b"another program\n").unwrap();
    assert_conflict(&run(job_of(&mut editor, "second\n")));
    assert_eq!(fs::read(&path).unwrap(), b"another program\n");
}

#[test]
fn a_thread_reports_as_the_seam_does() {
    let scratch = Scratch::new("thread");
    let path = scratch.file("notes.txt", b"old\n");
    let (tx, rx) = mpsc::channel();
    spawn(job(&path, "new\n"), move |report| tx.send(report).unwrap()).unwrap();
    let report = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_written(&report);
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Applies `report`, and returns the message with the state it leaves.
fn applied(editor: &mut Editor, report: SaveReport) -> String {
    apply(editor, report);
    assert!(!editor.current().1.is_saving());
    kept_messages(editor).pop().unwrap()
}

#[cfg(all(feature = "dev", any(target_os = "macos", target_os = "linux")))]
mod lists;
mod outcomes;
#[cfg(unix)]
mod unix;
