//! The tests of safe save, on a scratch directory with no window and, but for one
//! test, no thread.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use oso_core::buffer::{Baseline, Buffer, Replacement};
use oso_core::display_map::TabWidth;
use oso_core::editor::{Editor, SaveJob};
use oso_core::load::decode_all;

use crate::loader::{location, stamp_of};

use super::{
    AfterSave, PRIVATE_MODE, SaveError, SaveReport, Temporary, after_save_while_leaving, apply,
    at_leaving_deadline, prepare, run, run_with, spawn, unless_unsupported, write_text,
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

/// An editor that has loaded `path`, or found it missing, as the app does.
pub(crate) fn visit(path: &Path) -> Editor {
    let mut buffer = Buffer::visiting(path.to_path_buf());
    match fs::read(path) {
        Ok(content) => buffer.apply_load(decode_all(&content, TabWidth::default())),
        Err(_) => buffer.finish_empty(),
    }
    if location(path).is_ok() {
        buffer.set_baseline(baseline(path));
    }
    Editor::with_buffer(buffer)
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

#[test]
fn a_panic_ends_the_save_with_a_report() {
    let scratch = Scratch::new("panic-report");
    let path = scratch.file("notes.txt", b"old\n");
    let mut editor = visit(&path);
    let job = job_of(&mut editor, "new\n");
    let report = run_with(job, |_| panic!("a bug"));
    assert!(matches!(report.outcome, Err(SaveError::Panicked)));
    apply(&mut editor, report);
    let buffer = editor.current().1;
    assert!(!buffer.is_saving(), "not stuck");
    assert!(buffer.is_modified());
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
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
fn a_person_leaving_gets_the_text_back_if_the_save_fails() {
    use AfterSave::{Exit, KeepWaiting, StayOpen};
    // Failed, the socket's `exit`, another save running.
    assert_eq!(after_save_while_leaving(false, false, false), Exit);
    assert_eq!(after_save_while_leaving(false, false, true), KeepWaiting);
    assert_eq!(after_save_while_leaving(true, false, false), StayOpen);
    assert_eq!(
        after_save_while_leaving(true, false, true),
        StayOpen,
        "at once"
    );
    assert_eq!(after_save_while_leaving(true, true, false), Exit);
    assert_eq!(after_save_while_leaving(true, true, true), KeepWaiting);
    assert_eq!(after_save_while_leaving(false, true, false), Exit);
    assert_eq!(at_leaving_deadline(false), StayOpen);
    assert_eq!(at_leaving_deadline(true), Exit);
}

#[test]
fn a_report_says_whether_the_text_is_on_disk() {
    let scratch = Scratch::new("failed");
    let path = scratch.file("notes.txt", b"old\n");
    let written = run(job(&path, "new\n"));
    assert!(!written.failed());
    let job = job(&path, "other\n");
    fs::write(&path, b"another program\n").unwrap();
    assert!(run(job).failed());
}

#[test]
fn a_sync_a_file_system_cannot_do_is_said_and_others_fail_the_save() {
    use std::io::{Error, ErrorKind};
    assert!(unless_unsupported(Ok(())).unwrap().is_none());
    let unsupported = Error::from(ErrorKind::Unsupported);
    assert!(unless_unsupported(Err(unsupported)).unwrap().is_some());
    let invalid = Error::from(ErrorKind::InvalidInput);
    assert!(unless_unsupported(Err(invalid)).unwrap().is_some());
    assert!(unless_unsupported(Err(Error::other("the disk failed"))).is_err());
    assert!(unless_unsupported(Err(Error::from(ErrorKind::StorageFull))).is_err());
    #[cfg(unix)]
    {
        use super::sync_is_unsupported;
        #[cfg(target_vendor = "apple")]
        assert!(
            sync_is_unsupported(&Error::from_raw_os_error(45)),
            "ENOTSUP"
        );
        assert!(sync_is_unsupported(&Error::from_raw_os_error(22)), "EINVAL");
        assert!(!sync_is_unsupported(&Error::from_raw_os_error(5)), "EIO");
        assert!(
            !sync_is_unsupported(&Error::from_raw_os_error(28)),
            "ENOSPC"
        );
    }
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
fn a_buffer_with_no_baseline_says_so_and_does_not_call_it_a_change() {
    let scratch = Scratch::new("no-baseline-message");
    let path = scratch.file("notes.txt", b"old\n");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(b"old\n", TabWidth::default()));
    let mut editor = Editor::with_buffer(buffer);
    let job = job_of(&mut editor, "new\n");
    let report = run(job);
    assert!(
        matches!(report.outcome, Err(SaveError::UnknownLocation)),
        "{report:?}"
    );
    let message = applied(&mut editor, report);
    assert!(!message.contains("changed on disk"), "{message}");
    assert!(message.contains("could not be told"), "{message}");
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
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

/// Applies `report`, and returns the message with the state it leaves.
fn applied(editor: &mut Editor, report: SaveReport) -> String {
    apply(editor, report);
    assert!(!editor.current().1.is_saving());
    editor.messages().next_back().unwrap().to_owned()
}

const SECRET: &str = "S3CRET-TEXT\n";

#[test]
fn each_outcome_says_so_by_path_and_quotes_no_text() {
    let scratch = Scratch::new("messages");
    let path = scratch.file("notes.txt", b"old\n");
    let shown = path.display().to_string();

    let mut editor = visit(&path);
    let job = job_of(&mut editor, SECRET);
    assert_eq!(applied(&mut editor, run(job)), format!("Wrote {shown}"));
    assert!(!editor.current().1.is_modified());

    let mut editor = visit(&path);
    let job = job_of(&mut editor, "other\n");
    fs::write(&path, b"changed\n").unwrap();
    let message = applied(&mut editor, run(job));
    assert!(
        message.starts_with(&shown) && message.contains("changed on disk"),
        "{message}"
    );
    assert!(editor.current().1.is_modified());

    let mut editor = visit(&path);
    let job = job_of(&mut editor, SECRET);
    let buffer = job.buffer;
    let report = SaveReport::not_started(buffer, path.clone(), std::io::Error::other("no threads"));
    let message = applied(&mut editor, report);
    assert_eq!(message, format!("Cannot save {shown}: no threads"));
    assert!(editor.current().1.is_modified());
    for message in editor.messages() {
        assert!(!message.contains(SECRET.trim()), "{message}");
    }
}

#[test]
fn a_flush_that_failed_after_the_rename_counts_as_written_and_says_so() {
    let scratch = Scratch::new("unsynced");
    let path = scratch.file("notes.txt", b"old\n");
    let mut editor = visit(&path);
    let job = job_of(&mut editor, "new\n");
    let content = job.content;
    let report = SaveReport {
        buffer: job.buffer,
        path: path.clone(),
        outcome: Ok(Some(super::Written {
            content,
            stamp: stamp_of(&fs::metadata(&path).unwrap()).unwrap(),
            unsynced: Some(std::io::Error::other("no flush here")),
        })),
    };
    let message = applied(&mut editor, report);
    assert!(
        message.starts_with("Wrote") && message.contains("no flush here"),
        "{message}"
    );
    assert!(!editor.current().1.is_modified());
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

#[cfg(unix)]
mod unix;
