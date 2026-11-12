//! What a scan makes of what is not a plain file of this user's, on Unix (ADR 0064).
//!
//! Must not depend on: `app` or winit.

use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn a_link_for_a_meta_or_a_text_is_not_followed() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let outside = scratch.0.join("outside.txt");
    fs::write(&outside, "someone's text").unwrap();
    // The size matches, so only that it is a link can stop it being read.
    write_copy(
        &directory,
        STEM,
        &scratch.0.join("a"),
        "someone's text",
        None,
    );
    let text = directory.join(format!("{STEM}.txt"));
    fs::remove_file(&text).unwrap();
    symlink(&outside, &text).unwrap();
    assert_eq!(
        words(&scan(&request(&directory, Vec::new()), later())),
        [RowState::Damaged]
    );
    assert!(open_checked(&text).is_err());

    let meta = directory.join(format!("{STEM}.meta"));
    let elsewhere = scratch.0.join("elsewhere.meta");
    fs::rename(&meta, &elsewhere).unwrap();
    symlink(&elsewhere, &meta).unwrap();
    assert_eq!(
        words(&scan(&request(&directory, Vec::new()), later())),
        [RowState::Unknown]
    );
}

#[test]
fn a_hard_link_is_a_name_for_some_other_files_text() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let outside = scratch.0.join("key");
    fs::write(&outside, "text").unwrap();
    write_copy(&directory, STEM, &scratch.0.join("a"), "text", None);
    let text = directory.join(format!("{STEM}.txt"));
    fs::remove_file(&text).unwrap();
    fs::hard_link(&outside, &text).unwrap();
    assert_eq!(
        words(&scan(&request(&directory, Vec::new()), later())),
        [RowState::Damaged]
    );
    assert!(open_checked(&text).is_err());
}

/// What is no regular file is not opened, as a pipe would block for good. The flows have
/// a real pipe.
#[test]
fn what_is_not_a_regular_file_is_not_opened() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let meta = directory.join(format!("{STEM}.meta"));
    let text = directory.join(format!("{STEM}.txt"));
    fs::create_dir(&meta).unwrap();
    fs::create_dir(&text).unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(words(&found), [RowState::Unknown]);
    assert!(open_checked(&text).is_err());
}

#[test]
fn a_directory_others_may_reach_is_not_read_and_says_why() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    write_copy(&directory, STEM, &scratch.0.join("a"), "text", None);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    assert!(found.rows.is_empty());
    assert!(found.problem.is_some());
    assert!(found.list.text().to_string().contains("Not read:"));
}

#[test]
fn a_read_only_directory_can_still_be_read() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    write_copy(&directory, STEM, &scratch.0.join("a"), "text", None);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(words(&found), [RowState::NotOpen]);
    assert_eq!(found.problem, None);
}

/// Runs `chmod` with `args`, as this crate has no bindings of its own.
#[cfg(target_os = "macos")]
fn chmod(args: &[&std::ffi::OsStr]) {
    use oso_platform::process::{Executable, Sink, Spawn, spawn};

    let chmod = Executable::absolute("/bin/chmod").unwrap();
    let args: Vec<std::ffi::OsString> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let mut child = spawn(&Spawn {
        executable: &chmod,
        args: &args,
        cwd: None,
        project: None,
        inherit: &[],
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        set: &[],
        terminal: None,
    })
    .unwrap();
    assert!(child.wait().unwrap().success());
}

/// A leftover that took the list of the file it was to replace.
#[cfg(target_os = "macos")]
#[test]
fn deleting_removes_what_denies_deleting_it() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let text = directory.join(".oso-save-1-2-0.tmp");
    fs::write(&text, "text").unwrap();
    chmod(&[
        "+a".as_ref(),
        "everyone deny delete".as_ref(),
        text.as_ref(),
    ]);
    assert!(
        fs::remove_file(&text).is_err(),
        "the list stops a plain delete"
    );

    remove_copy(&text, None).unwrap();
    assert!(!text.exists());
}

/// When the directory stops the second try too, the file is left owner-only, never more open.
#[cfg(target_os = "macos")]
#[test]
fn a_leftover_that_cannot_be_deleted_is_left_owner_only() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let text = directory.join(".oso-save-1-3-0.tmp");
    fs::write(&text, "text").unwrap();
    fs::set_permissions(&text, fs::Permissions::from_mode(0o644)).unwrap();
    chmod(&[
        "+a".as_ref(),
        "everyone deny delete".as_ref(),
        text.as_ref(),
    ]);
    let entry = "everyone deny delete_child";
    chmod(&["+a".as_ref(), entry.as_ref(), directory.as_ref()]);
    let removed = remove_copy(&text, None);
    chmod(&["-a".as_ref(), entry.as_ref(), directory.as_ref()]);
    assert!(removed.is_err());
    assert_eq!(
        fs::metadata(&text).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
