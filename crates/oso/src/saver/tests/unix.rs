//! The tests of safe save that only make sense on Unix: modes, links and owners.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};

use super::super::kept_mode;
use super::*;

#[test]
fn the_mode_stays_from_private_to_executable() {
    let scratch = Scratch::new("modes");
    for wanted in [0o600, 0o640, 0o644, 0o664, 0o755, 0o700] {
        let path = scratch.file("file", b"old\n");
        set_mode(&path, wanted);
        assert_written(&run(job(&path, "new\n")));
        assert_eq!(mode(&path), wanted);
        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn the_temporary_file_is_private_and_a_new_name_while_it_fills() {
    let scratch = Scratch::new("private");
    let path = scratch.file("notes.txt", b"old\n");
    set_mode(&path, 0o644);
    let job = job(&path, "new\n");
    let (temporary, _file) = Temporary::create(scratch.path(), PRIVATE_MODE).unwrap();
    assert_eq!(mode(&temporary.path), 0o600);
    let name = temporary.path.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with(TEMPORARY_PREFIX), "{name}");
    assert!(!name.contains("notes"));
    let inode = fs::metadata(&path).unwrap().ino();
    drop(temporary);
    assert_written(&run(job));
    assert_ne!(
        fs::metadata(&path).unwrap().ino(),
        inode,
        "a new file, by rename"
    );
}

#[test]
fn modes_lose_special_bits_and_a_lost_group_gets_no_more_than_others() {
    assert_eq!(kept_mode(0o100_644, true), 0o644, "the file type");
    assert_eq!(kept_mode(0o4755, true), 0o755, "set-user-ID");
    assert_eq!(kept_mode(0o2755, true), 0o755, "set-group-ID");
    assert_eq!(kept_mode(0o640, true), 0o640);
    assert_eq!(kept_mode(0o640, false), 0o600);
    assert_eq!(kept_mode(0o660, false), 0o600);
    assert_eq!(
        kept_mode(0o644, false),
        0o644,
        "others could read it anyway"
    );
    assert_eq!(kept_mode(0o755, false), 0o755);
    assert_eq!(kept_mode(0o764, false), 0o744);
    assert_eq!(
        kept_mode(0o604, false),
        0o604,
        "nothing to take from the group"
    );
}

#[test]
fn saving_writes_through_a_link_and_the_link_stays() {
    let scratch = Scratch::new("link");
    fs::create_dir(scratch.path().join("real")).unwrap();
    let real = scratch.file("real/notes.txt", b"old\n");
    let absolute = scratch.path().join("absolute");
    let relative = scratch.path().join("relative");
    symlink(&real, &absolute).unwrap();
    symlink("real/notes.txt", &relative).unwrap();
    for (link, text) in [(&absolute, "one\n"), (&relative, "two\n")] {
        assert_written(&run(job(link, text)));
        assert!(fs::symlink_metadata(link).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), text);
    }
    assert_eq!(
        fs::read_link(&relative).unwrap(),
        Path::new("real/notes.txt")
    );
}

#[test]
fn the_temporary_file_goes_in_the_directory_of_the_file_itself() {
    let scratch = Scratch::new("link-dir");
    fs::create_dir(scratch.path().join("real")).unwrap();
    let real = scratch.file("real/notes.txt", b"old\n");
    let link = scratch.path().join("link");
    symlink(&real, &link).unwrap();
    let job = job(&link, "new\n");
    let prepared = prepare(&job).unwrap();
    assert_eq!(prepared.temporary.path.parent(), real.parent());
    assert_eq!(scratch.names(), ["link", "real"], "nothing beside the link");
}

#[test]
fn a_link_chain_and_a_link_to_a_missing_file_write_through() {
    let scratch = Scratch::new("missing-target");
    let target = scratch.path().join("target.txt");
    let inner = scratch.path().join("inner");
    let outer = scratch.path().join("outer");
    symlink("target.txt", &inner).unwrap();
    symlink("inner", &outer).unwrap();
    assert_written(&run(job(&outer, "new\n")));
    assert_eq!(fs::read(&target).unwrap(), b"new\n");
    assert!(fs::symlink_metadata(&outer).unwrap().is_symlink());
    assert!(fs::symlink_metadata(&inner).unwrap().is_symlink());
    assert_eq!(scratch.names(), ["inner", "outer", "target.txt"]);
}

#[test]
fn a_link_to_a_missing_directory_is_an_error() {
    let scratch = Scratch::new("missing-dir");
    let link = scratch.path().join("link");
    symlink("no-such-directory/file", &link).unwrap();
    let report = run(job(&link, "new\n"));
    assert!(
        matches!(report.outcome, Err(SaveError::Io(_))),
        "{report:?}"
    );
    assert_eq!(scratch.names(), ["link"]);
}

#[test]
fn a_loop_of_links_is_an_error() {
    let scratch = Scratch::new("loop");
    let link = scratch.path().join("link");
    symlink("link", &link).unwrap();
    let mut editor = Editor::with_buffer(Buffer::visiting(link));
    editor
        .buffer_mut(editor.window().buffer())
        .unwrap()
        .finish_empty();
    let job = job_of(&mut editor, "new\n");
    let report = run(job);
    assert!(
        matches!(report.outcome, Err(SaveError::Io(_))),
        "{report:?}"
    );
    assert_eq!(scratch.names(), ["link"]);
}

#[test]
fn a_link_swapped_in_for_the_file_is_a_conflict() {
    let scratch = Scratch::new("swapped");
    let path = scratch.file("notes.txt", b"old\n");
    let other = scratch.file("other.txt", b"other\n");
    let job = job(&path, "new\n");
    fs::remove_file(&path).unwrap();
    symlink(&other, &path).unwrap();
    assert_conflict(&run(job));
    assert_eq!(fs::read(&other).unwrap(), b"other\n");
}

#[test]
fn a_read_only_directory_is_an_error_and_changes_nothing() {
    let scratch = Scratch::new("read-only-dir");
    let directory = scratch.path().join("locked");
    fs::create_dir(&directory).unwrap();
    let path = directory.join("notes.txt");
    fs::write(&path, b"old\n").unwrap();
    let job = job(&path, "new\n");
    set_mode(&directory, 0o555);
    // Root ignores modes, so this cannot fail for it.
    let locked = fs::File::create(directory.join("probe")).is_err();
    let report = run(job);
    set_mode(&directory, 0o755);
    if locked {
        assert!(
            matches!(report.outcome, Err(SaveError::Io(_))),
            "{report:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }
}

#[test]
fn a_read_only_file_stays_as_it_is_though_a_rename_could_replace_it() {
    let scratch = Scratch::new("read-only-file");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    set_mode(&path, 0o444);
    // Root ignores modes, so this cannot fail for it.
    let locked = fs::OpenOptions::new().write(true).open(&path).is_err();
    let baseline_after_chmod = baseline(&path);
    assert_eq!(job.baseline, baseline_after_chmod, "a mode is not a change");
    let report = run(job);
    if locked {
        assert!(
            matches!(report.outcome, Err(SaveError::Io(_))),
            "{report:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"old\n");
        assert_eq!(scratch.names(), ["notes.txt"]);
    }
}

#[test]
fn another_hard_link_keeps_the_old_text_as_recorded_in_the_adr() {
    let scratch = Scratch::new("hard-link");
    let path = scratch.file("notes.txt", b"old\n");
    let other = scratch.path().join("other.txt");
    fs::hard_link(&path, &other).unwrap();
    assert_written(&run(job(&path, "new\n")));
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
    assert_eq!(fs::read(&other).unwrap(), b"old\n");
}

#[test]
fn a_name_that_is_not_utf8_saves_where_the_system_allows_one() {
    let scratch = Scratch::new("non-utf8");
    let name = std::ffi::OsStr::from_bytes(b"caf\xE9.txt");
    let path = scratch.path().join(name);
    // macOS refuses such a name.
    if fs::write(&path, b"old\n").is_err() {
        return;
    }
    assert_written(&run(job(&path, "new\n")));
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
    assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 1);
}
