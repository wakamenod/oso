//! The tests of how a save ends: what it reports, what it says, and what that means for
//! an exit under way.
//!
//! Must not depend on: `app`, winit or the GPU.

use super::*;

const SECRET: &str = "S3CRET-TEXT\n";

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
        use super::super::sync_is_unsupported;
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
fn a_save_that_outlived_a_leave_is_not_waited_for_again_but_others_are() {
    let scratch = Scratch::new("stuck");
    let path = scratch.file("notes.txt", b"old\n");
    let mut editor = visit(&path);
    let first = editor.window().buffer();
    assert!(!any_worth_waiting_for(&editor, &[]), "nothing runs");
    let job = job_of(&mut editor, "new\n");
    assert!(any_worth_waiting_for(&editor, &[]));
    assert!(!any_worth_waiting_for(&editor, &[first]), "stuck");

    let other = scratch.file("other.txt", b"old\n");
    let mut buffer = Buffer::visiting(other.clone());
    buffer.apply_load(decode_all(b"old\n", TabWidth::default()));
    buffer.set_baseline(baseline(&other));
    let second = editor.add_buffer(buffer);
    editor.show(second);
    let _other_job = job_of(&mut editor, "new\n");
    assert!(
        any_worth_waiting_for(&editor, &[first]),
        "another save runs"
    );
    assert!(!any_worth_waiting_for(&editor, &[first, second]));
    drop(job);
}

#[test]
fn what_the_disk_did_not_confirm_is_said_as_it_is_and_counts_as_written() {
    let scratch = Scratch::new("unsynced");
    let path = scratch.file("notes.txt", b"old\n");
    let cases = [
        (
            Unsynced::File(std::io::Error::other("no sync here")),
            "could lose it, and any file it replaced: no sync here",
        ),
        (
            Unsynced::Directory(std::io::Error::other("no flush here")),
            "could bring back what was there: no flush here",
        ),
    ];
    for (unsynced, said) in cases {
        let mut editor = visit(&path);
        let job = job_of(&mut editor, "new\n");
        let report = SaveReport {
            buffer: job.buffer,
            path: path.clone(),
            outcome: Ok(Some(super::super::Written {
                content: job.content,
                stamp: stamp_of(&fs::metadata(&path).unwrap()).unwrap(),
                unsynced: Some(unsynced),
            })),
        };
        let message = applied(&mut editor, report);
        assert!(message.starts_with("Wrote"), "{message}");
        assert!(message.ends_with(said), "{message}");
        assert!(!editor.current().1.is_modified());
    }
}

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
