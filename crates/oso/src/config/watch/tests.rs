//! The tests of the configuration's thread: what it reads and refuses, and that it reads the
//! file again when it changes (ADR 0108).
//!
//! Must not depend on: `app` or `dispatch`; these make files and take what the thread says.

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::{MAX_BYTES, Reader, Report};
use crate::config::{FILE_NAME, Source};
use crate::saver::tests::Scratch;

/// A thread reading `file`, and what wakes the test when it has something.
fn watch(file: &Path) -> (Reader, mpsc::Receiver<()>) {
    let watch = Reader::start(&Source::File(file.to_path_buf())).unwrap();
    let (woke, wakes) = mpsc::channel();
    watch.set_wake(move || {
        let _ = woke.send(());
    });
    (watch, wakes)
}

/// What the thread finds next.
fn next(watch: &Reader, wakes: &mpsc::Receiver<()>) -> Report {
    loop {
        wakes
            .recv_timeout(Duration::from_secs(10))
            .expect("the thread says nothing");
        if let Some(report) = watch.take() {
            return report;
        }
    }
}

fn refusal(report: Report) -> String {
    match report {
        Report::Refused(why) => why,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn reads_and_parses_the_file_and_reads_it_again_when_it_changes() {
    let scratch = Scratch::new("config-reads");
    let file = scratch.file(FILE_NAME, b"[keys.global]\n\"C-c a\" = \"undo\"\n");
    let (watch, wakes) = watch(&file);
    let Report::Read {
        loaded,
        group_writes,
    } = next(&watch, &wakes)
    else {
        panic!("not read");
    };
    assert!(!group_writes);
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert_eq!(loaded.keys.unwrap().len(), 1);

    fs::write(&file, b"[keys.global]\n\"C-c a\" = \"no-such-command\"\n").unwrap();
    watch.look_again();
    let Report::Read { loaded, .. } = next(&watch, &wakes) else {
        panic!("not read again");
    };
    assert_eq!(loaded.problems.len(), 1);

    fs::remove_file(&file).unwrap();
    watch.look_again();
    assert!(matches!(next(&watch, &wakes), Report::Missing));
}

#[test]
fn nothing_new_says_nothing() {
    let scratch = Scratch::new("config-quiet");
    let file = scratch.file(FILE_NAME, b"");
    let (watch, wakes) = watch(&file);
    assert!(matches!(next(&watch, &wakes), Report::Read { .. }));
    watch.look_again();
    assert!(wakes.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(watch.take().is_none());
}

#[test]
fn a_missing_file_is_the_defaults_and_no_file_is_no_thread() {
    let scratch = Scratch::new("config-missing");
    let (watch, wakes) = watch(&scratch.path().join(FILE_NAME));
    assert!(matches!(next(&watch, &wakes), Report::Missing));
    assert!(Reader::start(&Source::Nothing).is_none());
}

#[test]
fn refuses_what_is_not_a_small_regular_file_of_utf_8() {
    let scratch = Scratch::new("config-refuses");
    let directory = scratch.path().join("a-directory");
    fs::create_dir(&directory).unwrap();
    let large = scratch.file("large.toml", &vec![b'#'; MAX_BYTES as usize + 1]);
    let binary = scratch.file("binary.toml", b"x = \"\xff\"\n");
    let fits = scratch.file("fits.toml", &vec![b'#'; MAX_BYTES as usize]);
    for (file, why) in [
        (directory, "it is not a regular file"),
        (large, "it is larger than 256 KiB"),
        (binary, "it is not UTF-8 text"),
    ] {
        let (watch, wakes) = watch(&file);
        assert_eq!(refusal(next(&watch, &wakes)), why, "{file:?}");
    }
    let (watch, wakes) = watch(&fits);
    assert!(matches!(next(&watch, &wakes), Report::Read { .. }));
}

#[cfg(unix)]
#[test]
fn refuses_a_file_or_directory_others_can_change_and_warns_of_one_its_group_can() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
    let scratch = Scratch::new("config-writers");
    let file = scratch.file(FILE_NAME, b"");
    mode(&file, 0o646).unwrap();
    let (watch, wakes) = watch(&file);
    assert_eq!(refusal(next(&watch, &wakes)), "other users can change it");
    mode(&file, 0o664).unwrap();
    watch.look_again();
    assert!(matches!(
        next(&watch, &wakes),
        Report::Read {
            group_writes: true,
            ..
        }
    ));
    mode(&file, 0o644).unwrap();
    let shared = scratch.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let inside = shared.join(FILE_NAME);
    fs::write(&inside, b"").unwrap();
    mode(&shared, 0o777).unwrap();
    let (watch, wakes) = self::watch(&inside);
    let why = refusal(next(&watch, &wakes));
    mode(&shared, 0o755).unwrap();
    assert_eq!(why, "other users can change the directory it is in");
}

/// A link must not take the file out of a directory other users can change: the directory it is
/// named in counts as much as the one it is really in. Fixing the directory's mode is seen at
/// the next look asked for, though the file has not changed.
#[cfg(unix)]
#[test]
fn a_link_in_a_directory_others_can_change_is_refused_until_the_directory_is_fixed() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
    let scratch = Scratch::new("config-link-shared");
    let target = scratch.file("mine.toml", b"[keys.global]\nC-z = false\n");
    let shared = scratch.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let link = shared.join(FILE_NAME);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    mode(&shared, 0o777).unwrap();
    let (reader, wakes) = watch(&link);
    let why = refusal(next(&reader, &wakes));
    mode(&shared, 0o775).unwrap();
    assert_eq!(why, "other users can change the directory it is in");
    reader.look_again();
    let report = next(&reader, &wakes);
    mode(&shared, 0o755).unwrap();
    assert!(
        matches!(
            report,
            Report::Read {
                group_writes: true,
                ..
            }
        ),
        "{report:?}"
    );
}

/// Dotfile setups link the file from elsewhere: the file the link leads to is read and judged.
#[cfg(unix)]
#[test]
fn follows_a_link_and_judges_where_it_leads() {
    let scratch = Scratch::new("config-link");
    let real = scratch.path().join("dotfiles");
    fs::create_dir(&real).unwrap();
    let target = real.join(FILE_NAME);
    fs::write(&target, b"[keys.global]\nC-z = false\n").unwrap();
    let link = scratch.path().join(FILE_NAME);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let (watch, wakes) = watch(&link);
    let Report::Read { loaded, .. } = next(&watch, &wakes) else {
        panic!("not read");
    };
    assert_eq!(loaded.keys.unwrap().len(), 1);
}
