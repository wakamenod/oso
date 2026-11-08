//! The tests of safe save that only make sense on Unix: modes, links and owners.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};

use super::super::{TEMPORARY_PREFIX, attributes_moved, kept_mode, open_new};
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
fn a_relative_link_counts_from_its_own_directory_through_dot_dot() {
    let scratch = Scratch::new("dot-dot");
    fs::create_dir(scratch.path().join("links")).unwrap();
    fs::create_dir(scratch.path().join("real")).unwrap();
    let real = scratch.file("real/notes.txt", b"old\n");
    let link = scratch.path().join("links/link");
    symlink("../real/notes.txt", &link).unwrap();
    assert_written(&run(job(&link, "new\n")));
    assert_eq!(fs::read(&real).unwrap(), b"new\n");
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
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
fn a_loop_of_links_made_since_the_load_is_an_error() {
    let scratch = Scratch::new("loop");
    let file = scratch.file("notes.txt", b"old\n");
    let link = scratch.path().join("link");
    symlink(&file, &link).unwrap();
    let job = job(&link, "new\n");
    fs::remove_file(&link).unwrap();
    symlink("link", &link).unwrap();
    let report = run(job);
    assert!(
        matches!(report.outcome, Err(SaveError::Io(_))),
        "{report:?}"
    );
    assert_eq!(fs::read(&file).unwrap(), b"old\n");
    assert_eq!(scratch.names(), ["link", "notes.txt"]);
}

#[test]
fn a_link_swapped_in_for_the_file_is_not_written_through() {
    let scratch = Scratch::new("swapped");
    let path = scratch.file("notes.txt", b"old\n");
    let other = scratch.file("other.txt", b"other\n");
    let job = job(&path, "new\n");
    fs::remove_file(&path).unwrap();
    symlink(&other, &path).unwrap();
    let report = run(job);
    assert!(
        matches!(report.outcome, Err(SaveError::Elsewhere)),
        "{report:?}"
    );
    assert_eq!(fs::read(&other).unwrap(), b"other\n");
}

#[test]
fn a_link_swapped_in_during_the_write_is_not_written_through() {
    let scratch = Scratch::new("late-swap");
    let path = scratch.file("notes.txt", b"old\n");
    let other = scratch.file("other.txt", b"other\n");
    let job = job(&path, "new\n");
    let prepared = prepare(&job).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&other, &path).unwrap();
    assert!(matches!(prepared.commit(&job), Err(SaveError::Elsewhere)));
    assert_eq!(fs::read(&other).unwrap(), b"other\n");
    assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
    assert_eq!(scratch.names(), ["notes.txt", "other.txt"]);
}

#[test]
fn a_link_pointed_elsewhere_since_the_load_writes_nothing_at_the_new_place() {
    let scratch = Scratch::new("retarget");
    let first = scratch.file("first.txt", b"old\n");
    let existing = scratch.file("existing.txt", b"existing\n");
    let missing = scratch.path().join("missing.txt");
    let link = scratch.path().join("link");
    symlink(&first, &link).unwrap();
    for elsewhere in [&missing, &existing] {
        let job = job(&link, "text\n");
        fs::remove_file(&link).unwrap();
        symlink(elsewhere, &link).unwrap();
        let report = run(job);
        assert!(
            matches!(report.outcome, Err(SaveError::Elsewhere)),
            "{report:?}"
        );
        fs::remove_file(&link).unwrap();
        symlink(&first, &link).unwrap();
    }
    assert!(!missing.exists());
    assert_eq!(fs::read(&existing).unwrap(), b"existing\n");
    assert_eq!(fs::read(&first).unwrap(), b"old\n");
}

#[test]
fn a_regular_file_replaced_by_a_link_to_nothing_writes_nothing_there() {
    let scratch = Scratch::new("swap-dangling");
    let path = scratch.file("notes.txt", b"old\n");
    let missing = scratch.path().join("missing.txt");
    let job = job(&path, "new\n");
    fs::remove_file(&path).unwrap();
    symlink(&missing, &path).unwrap();
    let report = run(job);
    assert!(
        matches!(report.outcome, Err(SaveError::Elsewhere)),
        "{report:?}"
    );
    assert!(!missing.exists());
}

#[test]
fn a_directory_link_pointed_elsewhere_since_the_load_writes_nothing_there() {
    let scratch = Scratch::new("directory-link");
    fs::create_dir(scratch.path().join("one")).unwrap();
    fs::create_dir(scratch.path().join("two")).unwrap();
    let file = scratch.file("one/notes.txt", b"old\n");
    let link = scratch.path().join("current");
    symlink("one", &link).unwrap();
    let path = link.join("notes.txt");
    let job = job(&path, "new\n");
    fs::remove_file(&link).unwrap();
    symlink("two", &link).unwrap();
    let report = run(job);
    assert!(
        matches!(report.outcome, Err(SaveError::Elsewhere)),
        "{report:?}"
    );
    assert!(!scratch.path().join("two/notes.txt").exists());
    assert_eq!(fs::read(&file).unwrap(), b"old\n");
}

#[test]
fn a_directory_link_changed_during_the_write_loses_no_temporary_file() {
    let scratch = Scratch::new("late-directory-link");
    fs::create_dir(scratch.path().join("one")).unwrap();
    fs::create_dir(scratch.path().join("two")).unwrap();
    let file = scratch.file("one/notes.txt", b"old\n");
    let link = scratch.path().join("current");
    symlink("one", &link).unwrap();
    let job = job(&link.join("notes.txt"), "new\n");
    let prepared = prepare(&job).unwrap();
    fs::remove_file(&link).unwrap();
    symlink("two", &link).unwrap();
    assert!(matches!(prepared.commit(&job), Err(SaveError::Elsewhere)));
    assert_eq!(fs::read(&file).unwrap(), b"old\n");
    assert_eq!(fs::read_dir(scratch.path().join("one")).unwrap().count(), 1);
    assert_eq!(fs::read_dir(scratch.path().join("two")).unwrap().count(), 0);
}

#[test]
fn an_unmodified_buffer_whose_path_moved_on_is_told_when_its_file_has_gone() {
    let scratch = Scratch::new("gone-elsewhere");
    fs::create_dir(scratch.path().join("one")).unwrap();
    fs::create_dir(scratch.path().join("two")).unwrap();
    let file = scratch.file("one/notes.txt", b"the only copy\n");
    let link = scratch.path().join("current");
    symlink("one", &link).unwrap();
    let mut editor = visit(&link.join("notes.txt"));

    // The file is still where it was, so there is nothing to save.
    fs::remove_file(&link).unwrap();
    symlink("two", &link).unwrap();
    editor.start_save().unwrap();
    let report = run(editor.take_save_jobs().pop().unwrap());
    assert!(matches!(report.outcome, Ok(None)), "{report:?}");
    apply(&mut editor, report);

    fs::remove_file(&file).unwrap();
    editor.start_save().unwrap();
    let report = run(editor.take_save_jobs().pop().unwrap());
    assert!(
        matches!(report.outcome, Err(SaveError::Elsewhere)),
        "{report:?}"
    );
    assert_eq!(fs::read_dir(scratch.path().join("two")).unwrap().count(), 0);
}

#[test]
fn a_directory_made_since_a_new_file_was_visited_is_the_same_place() {
    let scratch = Scratch::new("made-directory");
    let directory = scratch.path().join("later");
    let path = directory.join("deep").join("new.txt");
    let job = job(&path, "new\n");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    assert_written(&run(job));
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
}

#[test]
fn a_deleted_file_gets_its_mode_back_and_no_wider() {
    let scratch = Scratch::new("deleted-mode");
    for (old, new) in [
        (0o600, 0o600),
        (0o640, 0o600),
        (0o644, 0o644),
        (0o755, 0o755),
    ] {
        let path = scratch.file("notes.txt", b"old\n");
        set_mode(&path, old);
        let job = job(&path, "new\n");
        fs::remove_file(&path).unwrap();
        assert_written(&run(job));
        assert_eq!(mode(&path), new, "from {old:o}");
        fs::remove_file(&path).unwrap();
    }
}

#[test]
fn set_user_id_does_not_survive_a_save() {
    let scratch = Scratch::new("setuid");
    let path = scratch.file("tool", b"old\n");
    set_mode(&path, 0o4755);
    assert_written(&run(job(&path, "new\n")));
    assert_eq!(mode(&path), 0o755);
}

#[test]
fn a_chmod_during_the_write_is_not_undone_by_the_rename() {
    let scratch = Scratch::new("late-chmod");
    let path = scratch.file("notes.txt", b"old\n");
    set_mode(&path, 0o644);
    let job = job(&path, "new\n");
    let prepared = prepare(&job).unwrap();
    set_mode(&path, 0o600);
    assert!(matches!(prepared.commit(&job), Err(SaveError::Conflict(_))));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
    assert_eq!(scratch.names(), ["notes.txt"]);
}

#[test]
fn only_a_changed_or_returned_file_makes_the_attributes_out_of_date() {
    let scratch = Scratch::new("attributes");
    let path = scratch.file("notes.txt", b"old\n");
    let before = fs::metadata(&path).unwrap();
    assert!(!attributes_moved(Some(&before), Some(&before)));
    assert!(attributes_moved(None, Some(&before)), "it came back");
    assert!(!attributes_moved(Some(&before), None), "deleted meanwhile");
    assert!(!attributes_moved(None, None));
}

#[test]
fn a_link_left_at_a_temporary_name_is_not_followed() {
    let scratch = Scratch::new("planted");
    let victim = scratch.file("victim.txt", b"victim\n");
    let missing = scratch.path().join("missing.txt");
    let planted = scratch.path().join("planted");
    let dangling = scratch.path().join("dangling");
    symlink(&victim, &planted).unwrap();
    symlink(&missing, &dangling).unwrap();
    assert!(open_new(&planted, PRIVATE_MODE).is_err());
    assert!(open_new(&dangling, PRIVATE_MODE).is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"victim\n");
    assert!(!missing.exists());
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

#[test]
fn another_file_of_the_same_time_and_size_renamed_in_is_a_conflict() {
    let scratch = Scratch::new("swapped");
    let path = scratch.file("notes.txt", b"old\n");
    let job = job(&path, "new\n");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let other = scratch.file("other.txt", b"odd\n");
    let file = fs::OpenOptions::new().write(true).open(&other).unwrap();
    file.set_modified(modified).unwrap();
    fs::rename(&other, &path).unwrap();
    assert_conflict(&run(job));
    assert_eq!(fs::read(&path).unwrap(), b"odd\n");
}
