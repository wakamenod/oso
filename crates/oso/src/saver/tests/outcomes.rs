//! The tests of how a save ends: what it reports, what it says, and what that means for
//! an exit under way.
//!
//! Must not depend on: `app`, winit or the GPU.

use super::*;

const SECRET: &str = "S3CRET-TEXT\n";

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
        assert!(message.starts_with("Wrote the file, but"), "{message}");
        assert!(message.contains(said), "{message}");
        assert!(message.ends_with(&path.display().to_string()), "{message}");
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
        message.ends_with(&shown) && message.starts_with("Not saved: the file changed on disk"),
        "{message}"
    );
    assert!(editor.current().1.is_modified());

    let mut editor = visit(&path);
    let job = job_of(&mut editor, SECRET);
    let buffer = job.buffer;
    let report = SaveReport::not_started(buffer, path.clone(), std::io::Error::other("no threads"));
    let message = applied(&mut editor, report);
    assert_eq!(message, format!("Not saved: no threads: {shown}"));
    assert!(editor.current().1.is_modified());
    for message in crate::commands::kept_messages(&editor) {
        assert!(!message.contains(SECRET.trim()), "{message}");
    }
}

#[test]
fn a_path_past_the_cap_leaves_what_happened_in_the_message() {
    let scratch = Scratch::new("long-path");
    let path = scratch.file("notes.txt", b"old\n");
    let mut editor = visit(&path);
    let job = job_of(&mut editor, "new\n");
    let long = PathBuf::from(format!("/{}", "long/".repeat(2000)));
    let error = std::io::Error::other("no threads");
    let report = SaveReport::not_started(job.buffer, long, error);
    let message = applied(&mut editor, report);
    assert!(
        message.starts_with("Not saved: no threads: /long/"),
        "{message}"
    );
    assert!(message.ends_with('…'), "{message}");
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
