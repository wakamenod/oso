//! The tests of the record of trusted projects' thread: what it reads and refuses, what it
//! changes in the file as it is, and that each write is read back (ADR 0114).
//!
//! Must not depend on: `app` or `dispatch`; these make files and take what the thread says.

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use oso_core::project::{RootIdentity, TrustChange, TrustList};

use super::{Record, TRUST_FILE_NAME, TrustFile, TrustLook};
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

/// A thread reading `file`, and what wakes the test when it has something.
fn watch(file: &Path) -> (TrustFile, mpsc::Receiver<()>) {
    let watch = TrustFile::start(Some(file.to_path_buf())).unwrap();
    let (woke, wakes) = mpsc::channel();
    watch.set_wake(move || {
        let _ = woke.send(());
    });
    (watch, wakes)
}

/// What the thread finds next.
fn next(watch: &TrustFile, wakes: &mpsc::Receiver<()>) -> TrustLook {
    loop {
        wakes
            .recv_timeout(Duration::from_secs(10))
            .expect("the thread says nothing");
        if let Some(look) = watch.take() {
            return look;
        }
    }
}

/// A root that can be recorded on this system.
fn root(scratch: &Scratch, name: &str) -> RootIdentity {
    RootIdentity {
        path: scratch.path().join(name),
        identity: cfg!(unix).then_some((1, 2)),
    }
}

/// Changes that trust `roots`.
fn trust(roots: &[RootIdentity]) -> Vec<TrustChange> {
    roots.iter().cloned().map(TrustChange::Trust).collect()
}

fn trusting(roots: &[RootIdentity]) -> TrustList {
    roots.iter().fold(TrustList::default(), |list, root| {
        list.apply(&TrustChange::Trust(root.clone())).unwrap()
    })
}

#[test]
fn a_missing_file_trusts_nothing_quietly() {
    let scratch = scratch("trust-missing");
    let (watch, wakes) = watch(&scratch.path().join(TRUST_FILE_NAME));
    let look = next(&watch, &wakes);
    assert_eq!(look.record, Some(Record::Read(TrustList::default())));
    assert!(look.said.is_empty());
}

#[test]
fn what_is_written_is_read_back_mode_0600() {
    let scratch = scratch("trust-write");
    let file = scratch.path().join("oso").join(TRUST_FILE_NAME);
    let (watch, wakes) = watch(&file);
    next(&watch, &wakes);
    let list = trusting(&[root(&scratch, "a"), root(&scratch, "b")]);
    watch.change(trust(&[root(&scratch, "a"), root(&scratch, "b")]));
    let look = next(&watch, &wakes);
    assert_eq!(look.record, Some(Record::Read(list.clone())));
    assert!(look.said.is_empty(), "{:?}", look.said);
    assert_eq!(fs::read_to_string(&file).unwrap(), list.format());
    assert!(!file.with_file_name("trusted-projects.part").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().unwrap()), 0o700, "made private");
    }
    // A change that changes nothing is read back too.
    watch.change(trust(&[root(&scratch, "b")]));
    assert_eq!(next(&watch, &wakes).record, Some(Record::Read(list)));
}

#[test]
fn a_line_it_cannot_read_is_counted_and_said() {
    let scratch = scratch("trust-skipped");
    let file = scratch.file(TRUST_FILE_NAME, b"# header\nrelative\tdev=1 ino=2\n");
    let (watch, wakes) = watch(&file);
    let look = next(&watch, &wakes);
    assert_eq!(look.record, Some(Record::Read(TrustList::default())));
    assert_eq!(
        look.said,
        ["trusted-projects: 1 line could not be read, and trust nothing"]
    );
}

#[test]
fn a_change_on_disk_is_read_and_the_same_text_said_once() {
    let scratch = scratch("trust-change");
    let file = scratch.file(TRUST_FILE_NAME, b"");
    let (watch, wakes) = watch(&file);
    next(&watch, &wakes);
    let list = trusting(&[root(&scratch, "a")]);
    fs::write(&file, list.format()).unwrap();
    watch.look_again();
    assert_eq!(
        next(&watch, &wakes).record,
        Some(Record::Read(list.clone()))
    );
    watch.look_again();
    assert!(
        wakes.recv_timeout(Duration::from_millis(300)).is_err(),
        "nothing new"
    );
}

#[cfg(unix)]
#[test]
fn a_file_a_group_can_change_trusts_nothing_and_is_not_written_over() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = scratch("trust-group");
    let list = trusting(&[root(&scratch, "a")]);
    let file = scratch.file(TRUST_FILE_NAME, list.format().as_bytes());
    fs::set_permissions(&file, fs::Permissions::from_mode(0o660)).unwrap();
    let (watch, wakes) = watch(&file);
    let look = next(&watch, &wakes);
    assert_eq!(look.record, Some(Record::Refused));
    assert_eq!(
        look.said,
        [
            "trusted-projects is not read, so no project is trusted: the users of a group can \
          change it, or a directory or link on the way to it"
        ]
    );
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    watch.look_again();
    let look = next(&watch, &wakes);
    assert_eq!(look.record, Some(Record::Read(list)));
    assert_eq!(look.said, ["trusted-projects is read again"]);
}

#[cfg(unix)]
#[test]
fn a_directory_others_can_change_is_not_written_in() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = scratch("trust-shared-dir");
    let shared = scratch.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    let file = shared.join(TRUST_FILE_NAME);
    let (watch, wakes) = watch(&file);
    next(&watch, &wakes);
    watch.change(trust(&[root(&scratch, "a")]));
    let look = next(&watch, &wakes);
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(look.record, Some(Record::Read(TrustList::default())));
    assert_eq!(
        look.said,
        [
            "trusted-projects was not changed, and still says what it said: other users can \
          change its directory, or the way to it"
        ]
    );
    assert!(!file.exists());
}

#[test]
fn a_change_is_made_in_the_file_as_it_is_so_a_line_written_by_hand_stays() {
    let scratch = scratch("trust-by-hand");
    let file = scratch.file(TRUST_FILE_NAME, b"");
    let (watch, wakes) = watch(&file);
    next(&watch, &wakes);
    // Written by hand, and not looked at yet.
    let by_hand = trusting(&[root(&scratch, "by-hand")]);
    fs::write(&file, by_hand.format()).unwrap();
    watch.change(trust(&[root(&scratch, "new")]));
    let both = trusting(&[root(&scratch, "by-hand"), root(&scratch, "new")]);
    loop {
        let look = next(&watch, &wakes);
        if look.record == Some(Record::Read(both.clone())) {
            break;
        }
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), both.format());
    watch.change(vec![TrustChange::Untrust(scratch.path().join("new"))]);
    assert_eq!(next(&watch, &wakes).record, Some(Record::Read(by_hand)));
}

#[cfg(unix)]
#[test]
fn a_file_refused_since_it_was_read_is_not_written_over() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = scratch("trust-refused-since");
    let file = scratch.file(TRUST_FILE_NAME, b"");
    let (watch, wakes) = watch(&file);
    next(&watch, &wakes);
    let text = "# by hand\n/w/not a line\tdev=x\n";
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o660)).unwrap();
    watch.change(trust(&[root(&scratch, "a")]));
    let mut said = Vec::new();
    loop {
        let look = next(&watch, &wakes);
        said.extend(look.said);
        if said
            .iter()
            .any(|note| note.starts_with("trusted-projects was not changed"))
        {
            break;
        }
    }
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
}

#[cfg(unix)]
#[test]
fn a_file_that_is_a_link_is_read_but_never_written_over() {
    let scratch = scratch("trust-link");
    let dotfiles = scratch.path().join("dotfiles");
    fs::create_dir(&dotfiles).unwrap();
    let target = dotfiles.join(TRUST_FILE_NAME);
    let list = trusting(&[root(&scratch, "a")]);
    fs::write(&target, list.format()).unwrap();
    let link = scratch.path().join(TRUST_FILE_NAME);
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let (watch, wakes) = watch(&link);
    assert_eq!(
        next(&watch, &wakes).record,
        Some(Record::Read(list.clone()))
    );
    watch.change(trust(&[root(&scratch, "b")]));
    let look = next(&watch, &wakes);
    assert_eq!(look.record, Some(Record::Read(list.clone())));
    assert_eq!(
        look.said,
        [
            "trusted-projects was not changed, and still says what it said: it is a link; change \
          the file it leads to by hand"
        ]
    );
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), list.format());
}

#[test]
fn changes_handed_over_as_oso_goes_are_made_before_it_does() {
    let scratch = scratch("trust-finish");
    let file = scratch.path().join(TRUST_FILE_NAME);
    let (watch, _wakes) = watch(&file);
    watch.change(trust(&[root(&scratch, "a")]));
    watch.finish(Duration::from_secs(10));
    drop(watch);
    let list = trusting(&[root(&scratch, "a")]);
    assert_eq!(fs::read_to_string(&file).unwrap(), list.format());
}

#[test]
fn a_file_replaced_since_it_was_read_is_not_written_over() {
    let scratch = scratch("trust-moved");
    let file = scratch.file(TRUST_FILE_NAME, b"");
    let seen = super::look(&file);
    // Another writer replaces it after the read.
    let theirs = trusting(&[root(&scratch, "theirs")]);
    std::thread::sleep(Duration::from_millis(20));
    fs::write(&file, theirs.format()).unwrap();
    let ours = trusting(&[root(&scratch, "ours")]);
    let written = super::write_file(&file, &ours, &seen).unwrap();
    assert!(matches!(written, super::Written::Moved));
    assert_eq!(fs::read_to_string(&file).unwrap(), theirs.format());
    let parts = fs::read_dir(scratch.path())
        .unwrap()
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            name.to_string_lossy().contains(".part")
        })
        .count();
    assert_eq!(parts, 0, "its part file goes");
}

#[test]
fn a_part_file_a_crash_left_goes_and_one_being_written_stays() {
    let scratch = scratch("trust-stale-part");
    let stale = scratch.file("trusted-projects.part.1", b"old");
    let fresh = scratch.file("trusted-projects.part.2", b"new");
    let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(an_hour_ago)
        .unwrap();
    let file = scratch.path().join(TRUST_FILE_NAME);
    let list = trusting(&[root(&scratch, "a")]);
    let written = super::write_file(&file, &list, &super::look(&file)).unwrap();
    assert!(matches!(written, super::Written::Replaced));
    assert!(!stale.exists());
    assert!(fresh.exists());
}
