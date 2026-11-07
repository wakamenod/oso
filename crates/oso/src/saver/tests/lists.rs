//! The tests of what a save does with real access lists and a real quarantine flag. They are set
//! by the system's own tools, as this crate has no bindings of its own, and the dev feature is what
//! lets a test start a process (ADR 0067).
//!
//! Must not depend on: `app`, winit or the GPU.

use std::ffi::OsString;

use oso_platform::file_meta::AccessList;
use oso_platform::process::{Executable, Sink, Spawn, spawn};

use super::*;

/// Runs a tool that must succeed.
fn tool(program: &str, args: &[&std::ffi::OsStr]) {
    let tool = Executable::absolute(program).unwrap();
    let args: Vec<OsString> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let mut child = spawn(&Spawn {
        executable: &tool,
        args: &args,
        cwd: None,
        project: None,
        inherit: &[],
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
    })
    .unwrap();
    assert!(child.wait().unwrap().success(), "{program} {args:?}");
}

fn list(path: &Path) -> AccessList {
    AccessList::at(path).unwrap()
}

#[cfg(target_os = "macos")]
mod macos {
    use oso_platform::file_meta::Quarantine;

    use super::*;

    #[track_caller]
    fn assert_list_error(report: &SaveReport) {
        assert!(
            matches!(report.outcome, Err(SaveError::ListForbidsReplacing(_))),
            "{:?}",
            report.outcome
        );
    }

    fn add(path: &Path, entry: &str) {
        tool(
            "/bin/chmod",
            &["+a".as_ref(), entry.as_ref(), path.as_ref()],
        );
    }

    fn take_off(path: &Path, entry: &str) {
        tool(
            "/bin/chmod",
            &["-a".as_ref(), entry.as_ref(), path.as_ref()],
        );
    }

    #[test]
    fn a_files_list_stays_through_a_save_exactly() {
        let scratch = Scratch::new("list-stays");
        let path = scratch.file("notes.txt", b"old\n");
        set_mode(&path, 0o600);
        add(&path, "everyone allow read");
        add(&path, "everyone deny execute");
        let before = list(&path);
        assert_written(&run(job(&path, "new\n")));
        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        assert_eq!(list(&path), before);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(scratch.names(), ["notes.txt"]);
    }

    #[test]
    fn a_files_list_is_all_it_has_after_a_save_whatever_its_directory_gives() {
        let scratch = Scratch::new("list-only");
        let path = scratch.file("notes.txt", b"old\n");
        add(&path, "everyone allow execute");
        let before = list(&path);
        add(scratch.path(), "everyone allow list,file_inherit");
        assert_written(&run(job(&path, "new\n")));
        assert_eq!(list(&path), before);
    }

    #[test]
    fn a_file_with_no_list_takes_none_from_its_directory_in_a_save() {
        let scratch = Scratch::new("list-none");
        let path = scratch.file("notes.txt", b"old\n");
        let before = list(&path);
        add(scratch.path(), "everyone allow list,file_inherit");
        assert_written(&run(job(&path, "new\n")));
        assert_eq!(list(&path), before);
    }

    #[test]
    fn a_new_file_gets_what_its_directory_says() {
        let scratch = Scratch::new("list-new");
        add(scratch.path(), "everyone allow list,file_inherit");
        let path = scratch.path().join("new.txt");
        assert_written(&run(job(&path, "new\n")));
        let control = scratch.file("control.txt", b"");
        assert_eq!(list(&path), list(&control));
        assert_ne!(list(&path), list(scratch.path()), "as a file takes it");
        assert_eq!(mode(&path), mode(&control));
    }

    #[test]
    fn a_list_changed_during_the_write_is_kept() {
        let scratch = Scratch::new("list-late");
        let path = scratch.file("notes.txt", b"old\n");
        let job = job(&path, "new\n");
        let prepared = prepare(&job).unwrap();
        add(&path, "everyone allow read");
        let changed = list(&path);
        assert!(matches!(prepared.commit(&job), Err(SaveError::Conflict)));
        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert_eq!(list(&path), changed);
        assert_eq!(scratch.names(), ["notes.txt"]);
    }

    #[test]
    fn a_file_whose_list_denies_deleting_it_is_kept_and_nothing_is_left() {
        let scratch = Scratch::new("list-deny-delete");
        let path = scratch.file("notes.txt", b"old\n");
        add(&path, "everyone deny delete");
        let report = run(job(&path, "new\n"));
        assert_list_error(&report);
        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert_eq!(scratch.names(), ["notes.txt"]);
        take_off(&path, "everyone deny delete");
    }

    #[test]
    fn a_new_file_in_a_directory_that_passes_on_denying_deleting_is_refused_and_nothing_is_left() {
        let scratch = Scratch::new("list-new-deny");
        let entry = "everyone deny delete,file_inherit,only_inherit";
        add(scratch.path(), entry);
        let path = scratch.path().join("new.txt");
        let report = run(job(&path, "new\n"));
        take_off(scratch.path(), entry);
        assert_list_error(&report);
        assert_eq!(scratch.names(), Vec::<String>::new());
    }

    #[test]
    fn a_denial_that_reaches_only_others_still_lets_the_save_through() {
        let scratch = Scratch::new("list-deny-other");
        let path = scratch.file("notes.txt", b"old\n");
        add(&path, "user:nobody deny delete");
        let before = list(&path);
        assert!(before.denies_deleting());
        assert_written(&run(job(&path, "new\n")));
        assert_eq!(list(&path), before);
    }

    #[test]
    fn a_list_that_cannot_be_read_stops_the_save_and_the_file_stays() {
        let scratch = Scratch::new("list-hidden");
        let path = scratch.file("notes.txt", b"old\n");
        let job = job(&path, "new\n");
        add(&path, "everyone deny readsecurity");
        let report = run(job);
        // A list hidden from its owner is replaced by `chmod`, whatever it held.
        add(&path, "everyone allow read");
        assert!(report.failed());
        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert_eq!(scratch.names(), ["notes.txt"]);
    }

    fn quarantine(path: &Path) {
        tool(
            "/usr/bin/xattr",
            &[
                "-w".as_ref(),
                "com.apple.quarantine".as_ref(),
                "0081;00000000;oso;".as_ref(),
                path.as_ref(),
            ],
        );
    }

    #[test]
    fn the_quarantine_flag_stays_through_a_save() {
        let scratch = Scratch::new("quarantine");
        let path = scratch.file("notes.txt", b"old\n");
        quarantine(&path);
        let before = Quarantine::at(&path).unwrap();
        assert!(before.is_some());
        let report = run(job(&path, "new\n"));
        assert_written(&report);
        assert!(matches!(
            report.outcome,
            Ok(Some(ref written)) if written.quarantine_lost.is_none()
        ));
        assert_eq!(Quarantine::at(&path).unwrap(), before);
    }

    #[test]
    fn a_quarantine_flag_that_cannot_be_read_is_said_and_the_save_goes_on() {
        let scratch = Scratch::new("quarantine-lost");
        let path = scratch.file("notes.txt", b"old\n");
        quarantine(&path);
        add(&path, "everyone deny readextattr");
        let before = list(&path);
        let report = run(job(&path, "new\n"));
        assert_written(&report);
        assert!(matches!(
            report.outcome,
            Ok(Some(ref written)) if written.quarantine_lost.is_some()
        ));
        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        assert_eq!(list(&path), before);
    }

    #[test]
    fn a_new_files_temporary_file_never_holds_what_its_directory_gives() {
        let scratch = Scratch::new("list-temporary");
        add(scratch.path(), "everyone allow list,file_inherit");
        let (_temporary, file) = Temporary::create_private(scratch.path()).unwrap();
        assert_eq!(AccessList::take_from(&file).unwrap(), AccessList::default());
    }

    #[test]
    fn a_temporary_file_that_cannot_be_removed_is_left_owner_only_and_never_more_open() {
        let scratch = Scratch::new("list-stuck");
        let (temporary, file) = Temporary::create(scratch.path(), 0o644).unwrap();
        drop(file);
        let path = temporary.path().to_owned();
        set_mode(&path, 0o644);
        add(&path, "everyone deny delete");
        add(scratch.path(), "everyone deny delete_child");
        drop(temporary);
        let left = fs::symlink_metadata(&path).is_ok();
        take_off(scratch.path(), "everyone deny delete_child");
        assert!(left, "the directory stopped the second try too");
        assert_eq!(mode(&path), 0o600, "shut before its list went");
        assert_eq!(list(&path), AccessList::default());
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    /// Runs `setfacl`, or says it is not here. Only outside CI may that pass as nothing to test.
    fn setfacl(args: &[&std::ffi::OsStr]) -> bool {
        let program = "/usr/bin/setfacl";
        if !Path::new(program).exists() {
            assert!(
                std::env::var_os("CI").is_none(),
                "these tests need setfacl in CI"
            );
            return false;
        }
        tool(program, args);
        true
    }

    #[test]
    fn a_files_list_stays_through_a_save_and_its_group_gets_nothing_new() {
        let scratch = Scratch::new("list-stays");
        let path = scratch.file("notes.txt", b"old\n");
        set_mode(&path, 0o600);
        if !setfacl(&["-m".as_ref(), "g::---,u:12345:rw".as_ref(), path.as_ref()]) {
            return;
        }
        assert_eq!(mode(&path), 0o660, "the group bits are the mask");
        let before = list(&path);
        assert_written(&run(job(&path, "new\n")));
        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        assert_eq!(list(&path), before);
        assert_eq!(mode(&path), 0o660);
    }

    #[test]
    fn a_file_with_no_list_takes_none_from_its_directory_in_a_save() {
        let scratch = Scratch::new("list-none");
        let path = scratch.file("notes.txt", b"old\n");
        set_mode(&path, 0o640);
        let before = list(&path);
        if !setfacl(&[
            "-m".as_ref(),
            "d:u:12345:rw".as_ref(),
            scratch.path().as_ref(),
        ]) {
            return;
        }
        assert_written(&run(job(&path, "new\n")));
        assert_eq!(list(&path), before);
        assert_eq!(mode(&path), 0o640);
    }

    #[test]
    fn a_new_file_gets_what_its_directory_says() {
        let scratch = Scratch::new("list-new");
        if !setfacl(&[
            "-m".as_ref(),
            "d:u:12345:rw".as_ref(),
            scratch.path().as_ref(),
        ]) {
            return;
        }
        let path = scratch.path().join("new.txt");
        assert_written(&run(job(&path, "new\n")));
        let control = scratch.file("control.txt", b"");
        assert_eq!(list(&path), list(&control));
        assert_eq!(mode(&path), mode(&control));
    }

    #[test]
    fn a_list_changed_during_the_write_is_kept_though_the_mode_says_nothing() {
        let scratch = Scratch::new("list-late");
        let path = scratch.file("notes.txt", b"old\n");
        set_mode(&path, 0o600);
        if !setfacl(&["-m".as_ref(), "u:12345:r,m::rw".as_ref(), path.as_ref()]) {
            return;
        }
        let job = job(&path, "new\n");
        let prepared = prepare(&job).unwrap();
        let before = mode(&path);
        // `-n` keeps the mask, and so the mode, as it was: only the list can tell.
        setfacl(&[
            "-n".as_ref(),
            "-m".as_ref(),
            "u:12345:rw".as_ref(),
            path.as_ref(),
        ]);
        assert_eq!(mode(&path), before, "nothing but the list changed");
        assert!(matches!(prepared.commit(&job), Err(SaveError::Conflict)));
        assert_eq!(fs::read(&path).unwrap(), b"old\n");
    }
}
