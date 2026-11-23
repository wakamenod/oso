//! The native trust judge's table, and the start thread's look at real directories (ADR 0165,
//! part 4a).
//!
//! Must not depend on: `app`.

use std::path::{Path, PathBuf};

use oso_core::project::TrustList;

use super::{Judged, Listed, NotTrusted, Record, judge};
use crate::git::TrustedRoot;

#[cfg(not(windows))]
const ROOT: &str = "/nowhere/oso-native-trust";
#[cfg(not(windows))]
const OTHER: &str = "/nowhere/oso-native-trust-2";
#[cfg(not(windows))]
const PARENT: &str = "/nowhere";
#[cfg(windows)]
const ROOT: &str = r"C:\nowhere\oso-native-trust";
#[cfg(windows)]
const OTHER: &str = r"C:\nowhere\oso-native-trust-2";
#[cfg(windows)]
const PARENT: &str = r"C:\nowhere";

fn open() -> TrustedRoot {
    TrustedRoot::for_tests(PathBuf::from(ROOT))
}

/// A record that holds `path` with `identity`, as `trust-project` writes it: with no identity
/// where the system has none.
fn record(path: &Path, identity: Option<(u64, u64)>) -> TrustList {
    let line = match identity.filter(|_| cfg!(unix)) {
        Some((dev, ino)) => format!("{}\tdev={dev} ino={ino}\n", path.display()),
        None => format!("{}\n", path.display()),
    };
    let (list, skipped) = TrustList::parse(&line);
    assert_eq!(skipped, 0, "{line:?}");
    list
}

/// What `judge` says at `ROOT`, as a word.
fn said(
    open: Option<Option<TrustedRoot>>,
    record: Option<Record<'_>>,
    may_wait: bool,
) -> Result<&'static str, NotTrusted> {
    judge(true, Path::new(ROOT), open, record, may_wait).map(|judged| match judged {
        Judged::Open(_) => "open",
        Judged::Listed(_) => "listed",
        Judged::Waits => "waits",
    })
}

#[test]
fn an_open_buffer_of_the_project_decides_whatever_the_record_says() {
    let listed = record(Path::new(ROOT), Some((1, 2)));
    let none = TrustList::default();
    for record in [
        None,
        Some(Record::Read(&listed)),
        Some(Record::Read(&none)),
        Some(Record::Unread),
        Some(Record::Refused),
    ] {
        assert_eq!(said(Some(Some(open())), record, true), Ok("open"));
        assert_eq!(
            said(Some(None), record, true),
            Err(NotTrusted::Untrusted),
            "open and not trusted refuses, the record holding it or not: {record:?}"
        );
        let revoked = open();
        revoked.revoke();
        assert_eq!(
            said(Some(Some(revoked)), record, true),
            Err(NotTrusted::Untrusted),
            "trust taken back refuses: {record:?}"
        );
    }
}

#[test]
fn with_none_open_the_record_decides_by_the_roots_path() {
    let listed = record(Path::new(ROOT), Some((1, 2)));
    assert_eq!(said(None, Some(Record::Read(&listed)), true), Ok("listed"));
    let other = record(Path::new(OTHER), Some((1, 2)));
    assert_eq!(
        said(None, Some(Record::Read(&other)), true),
        Err(NotTrusted::Untrusted)
    );
    let below = record(Path::new(PARENT), Some((1, 2)));
    assert_eq!(
        said(None, Some(Record::Read(&below)), true),
        Err(NotTrusted::Untrusted),
        "a trusted parent trusts no project below it"
    );
    assert_eq!(
        said(None, None, true),
        Err(NotTrusted::NotOpen),
        "a new start never asks the record"
    );
}

#[test]
fn a_record_not_read_waits_only_while_starts_may_wait_and_then_fails_closed() {
    assert_eq!(said(None, Some(Record::Unread), true), Ok("waits"));
    assert_eq!(
        said(None, Some(Record::Unread), false),
        Err(NotTrusted::NoRecord)
    );
    assert_eq!(
        said(None, Some(Record::Refused), true),
        Err(NotTrusted::NoRecord),
        "a refused record is read: nothing waits for it"
    );
}

#[test]
fn below_its_root_it_never_runs_natively() {
    let listed = record(Path::new(ROOT), Some((1, 2)));
    for (open, record) in [
        (Some(Some(open())), None),
        (None, Some(Record::Read(&listed))),
        (None, Some(Record::Unread)),
    ] {
        assert_eq!(
            judge(false, Path::new(ROOT), open, record, true).unwrap_err(),
            NotTrusted::NotAtRoot
        );
    }
}

/// A directory of its own, removed at the end.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "oso-native-trust-{name}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    ));
    std::fs::create_dir(&dir).unwrap();
    Scratch(dir)
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    crate::git::identity(&std::fs::symlink_metadata(path).unwrap())
}

/// The start thread's look at `root` as the record `list` holds it.
fn look(list: &TrustList, root: &Path) -> Option<TrustedRoot> {
    match judge(true, root, None, Some(Record::Read(list)), false) {
        Ok(Judged::Listed(listed)) => listed.look(),
        other => panic!("not judged by the record: {other:?}"),
    }
}

#[test]
fn the_start_thread_trusts_the_root_only_as_the_directory_recorded() {
    let scratch = scratch("look");
    let root = scratch.0.join("proj");
    std::fs::create_dir(&root).unwrap();
    let list = record(&root, identity(&root));
    let trusted = look(&list, &root).expect("the directory recorded");
    assert_eq!(trusted.path(), root);
    assert_eq!(trusted.identity(), identity(&root));
    assert!(!trusted.revoked());
    // Made again at the same path: another directory.
    let kept = scratch.0.join("kept");
    std::fs::create_dir(&kept).unwrap();
    std::fs::rename(&root, scratch.0.join("old")).unwrap();
    std::fs::create_dir(&root).unwrap();
    if identity(&root).is_some() {
        assert!(look(&list, &root).is_none(), "a directory made again");
    }
    // Gone.
    std::fs::remove_dir(&root).unwrap();
    assert!(look(&list, &root).is_none(), "nothing at the root");
    // A file where the root was, even as the one recorded.
    std::fs::write(&root, b"x").unwrap();
    let as_file = record(&root, identity(&root));
    assert!(look(&as_file, &root).is_none(), "a file is no root");
}

#[cfg(unix)]
#[test]
fn a_link_at_the_root_is_not_the_directory_it_leads_to() {
    let scratch = scratch("link");
    let real = scratch.0.join("real");
    std::fs::create_dir(&real).unwrap();
    let link = scratch.0.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    // The record holds the link's path with the directory's identity.
    let list = record(&link, identity(&real));
    assert!(look(&list, &link).is_none());
}

#[test]
fn a_listed_root_carries_the_record_as_read() {
    let list = record(Path::new(ROOT), Some((1, 2)));
    let Ok(Judged::Listed(Listed {
        list: carried,
        root,
    })) = judge(true, Path::new(ROOT), None, Some(Record::Read(&list)), true)
    else {
        panic!("not listed");
    };
    assert_eq!(carried, list);
    assert_eq!(root, Path::new(ROOT));
}
