//! The tests of the configuration's thread: what it reads and refuses, and that it reads the
//! file again when it changes (ADR 0108).
//!
//! Must not depend on: `app` or `dispatch`; these make files and take what the thread says.

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::{MAX_BYTES, Reader, Report, Shared, deliver};
use crate::config::{FILE_NAME, Source};
use crate::saver::tests::Scratch;

/// A scratch directory of this user's alone, mode 0700, whatever the umask.
fn scratch(test: &str) -> Scratch {
    let scratch = Scratch::new(test);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    scratch
}

/// A file in `scratch` with `content`, mode 0644, whatever the umask.
fn file(scratch: &Scratch, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = scratch.file(name, content);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    path
}

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
        if let Some((report, _)) = watch.take() {
            return report;
        }
    }
}

#[test]
fn only_the_first_look_taken_as_it_was_is_the_launchs() {
    let refused = || Report::Refused("why".to_owned());
    let shared = Shared::default();
    deliver(&shared, refused());
    assert!(matches!(shared.take(), Some((Report::Refused(_), true))));
    deliver(&shared, Report::Missing);
    assert!(matches!(shared.take(), Some((Report::Missing, false))));
    assert!(shared.take().is_none());

    // A first look replaced before it was taken: the file found fixed is not the launch's.
    let shared = Shared::default();
    deliver(&shared, refused());
    deliver(&shared, Report::Missing);
    assert!(matches!(shared.take(), Some((Report::Missing, false))));
}

fn refusal(report: Report) -> String {
    match report {
        Report::Refused(why) => why,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn reads_and_parses_the_file_and_reads_it_again_when_it_changes() {
    let scratch = scratch("config-reads");
    let file = file(
        &scratch,
        FILE_NAME,
        b"[keys.global]\n\"C-c a\" = \"undo\"\n",
    );
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
    let scratch = scratch("config-quiet");
    let file = file(&scratch, FILE_NAME, b"");
    let (watch, wakes) = watch(&file);
    assert!(matches!(next(&watch, &wakes), Report::Read { .. }));
    watch.look_again();
    assert!(wakes.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(watch.take().is_none());
}

#[test]
fn a_missing_file_is_the_defaults_and_no_file_is_no_thread() {
    let scratch = scratch("config-missing");
    let (watch, wakes) = watch(&scratch.path().join(FILE_NAME));
    assert!(matches!(next(&watch, &wakes), Report::Missing));
    assert!(Reader::start(&Source::Nothing).is_none());
}

#[test]
fn refuses_what_is_not_a_small_regular_file_of_utf_8() {
    let scratch = scratch("config-refuses");
    let directory = scratch.path().join("a-directory");
    fs::create_dir(&directory).unwrap();
    let large = file(&scratch, "large.toml", &vec![b'#'; MAX_BYTES as usize + 1]);
    let binary = file(&scratch, "binary.toml", b"x = \"\xff\"\n");
    let fits = file(&scratch, "fits.toml", &vec![b'#'; MAX_BYTES as usize]);
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
    let scratch = scratch("config-writers");
    let file = file(&scratch, FILE_NAME, b"");
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
    assert_eq!(
        why,
        "other users can change a directory or link on the way to it"
    );
}

/// A link must not take the file out of a directory other users can change: the directory it is
/// named in counts as much as the one it is really in. Fixing the directory's mode is seen at
/// the next look asked for, though the file has not changed.
#[cfg(unix)]
#[test]
fn a_link_in_a_directory_others_can_change_is_refused_until_the_directory_is_fixed() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
    let scratch = scratch("config-link-shared");
    let target = file(&scratch, "mine.toml", b"[keys.global]\nC-z = false\n");
    let shared = scratch.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let link = shared.join(FILE_NAME);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    mode(&shared, 0o777).unwrap();
    let (reader, wakes) = watch(&link);
    let why = refusal(next(&reader, &wakes));
    mode(&shared, 0o775).unwrap();
    assert_eq!(
        why,
        "other users can change a directory or link on the way to it"
    );
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
    let scratch = scratch("config-link");
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

#[test]
fn a_look_asked_for_says_a_refusal_or_a_rewrite_again_only_if_something_changed() {
    let scratch = scratch("config-once");
    let large = file(&scratch, "large.toml", &vec![b'#'; MAX_BYTES as usize + 1]);
    let (reader, wakes) = watch(&large);
    assert!(matches!(next(&reader, &wakes), Report::Refused(_)));
    reader.look_again();
    assert!(
        wakes.recv_timeout(Duration::from_millis(300)).is_err(),
        "said once"
    );

    let config = file(
        &scratch,
        FILE_NAME,
        b"[keys.global]\n\"C-c a\" = \"nothing\"\n",
    );
    let (reader, wakes) = watch(&config);
    assert!(matches!(next(&reader, &wakes), Report::Read { .. }));
    fs::write(&config, b"[keys.global]\n\"C-c a\" = \"nothing\"\n").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    reader.look_again();
    assert!(
        wakes.recv_timeout(Duration::from_millis(300)).is_err(),
        "the same text, its problems said once"
    );
}

/// A file whose name says it may hold secrets has its problems placed, and none of its text
/// quoted (ADR 0044).
#[test]
fn a_file_that_may_hold_secrets_has_no_text_quoted() {
    let scratch = scratch("config-secret");
    let named = file(
        &scratch,
        "credentials.toml",
        b"[keys.global]\n\"C-c a\" = \"hunter2\"\n",
    );
    let (reader, wakes) = watch(&named);
    let Report::Read { loaded, .. } = next(&reader, &wakes) else {
        panic!("not read");
    };
    assert_eq!(loaded.problems.len(), 1);
    assert_eq!(
        (loaded.problems[0].line, loaded.problems[0].column),
        (2, 11)
    );
    assert!(!loaded.problems[0].message.contains("hunter2"));
}
