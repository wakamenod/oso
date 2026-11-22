//! The index's lock over a temporary directory: how it is named, what a stop or a failure
//! leaves noted, and taking away only the file noted.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use std::path::{Path, PathBuf};

use super::{Lock, found, left, named};

/// An empty directory of its own for `name`, with a `.git` in it.
fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oso-lock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    dir
}

fn lock(dir: &Path) -> Lock {
    Lock::at(&dir.join(".git").join("index"), dir)
}

fn plant(dir: &Path) {
    std::fs::write(dir.join(".git").join("index.lock"), "").unwrap();
}

/// How a lock under the top is named.
fn shown() -> String {
    Path::new(".git").join("index.lock").display().to_string()
}

#[test]
fn a_lock_is_named_from_the_top_or_whole_outside_it() {
    let dir = dir("named");
    assert_eq!(lock(&dir).shown(), shown());
    let elsewhere = dir.join("other");
    let outside = Lock::at(&elsewhere.join("index"), &dir.join("tree"));
    assert_eq!(
        outside.shown(),
        elsewhere.join("index.lock").display().to_string()
    );
    // A name git gives is escaped on one row.
    let odd = Lock::at(&dir.join("a\nb").join("index"), &dir);
    assert!(!odd.shown().contains('\n'), "{}", odd.shown());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_stop_notes_a_lock_that_was_not_there_or_another_file() {
    let dir = dir("stop");
    let lock = lock(&dir);
    assert_eq!(lock.after(None, true, false), None, "nothing left");
    plant(&dir);
    let noted = lock.after(None, true, false).expect("a stop left it");
    assert!(noted.stopped);
    // The same file as the run began: the stop did not leave it.
    let before = lock.look();
    assert_eq!(lock.after(before, true, false), None);
    // Another file in its place: a stop left that one.
    std::fs::remove_file(dir.join(".git/index.lock")).unwrap();
    let keep = dir.join("keep");
    std::fs::write(&keep, "").unwrap();
    plant(&dir);
    #[cfg(unix)]
    assert!(lock.after(before, true, false).is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failure_notes_a_lock_only_when_its_output_names_one_that_is_there() {
    let dir = dir("failed");
    let lock = lock(&dir);
    assert_eq!(lock.after(None, false, true), None, "named, but not there");
    plant(&dir);
    assert_eq!(lock.after(None, false, false), None, "there, but not named");
    assert_eq!(
        lock.after(None, false, true),
        None,
        "not there as the run began: another git made it meanwhile"
    );
    let before = lock.look();
    let noted = lock.after(before, false, true).expect("named and there");
    assert!(!noted.stopped);
    assert!(named(
        "fatal: Unable to create '/r/.git/index.lock': File exists."
    ));
    assert!(named("致命的エラー: '/r/.git/index.lock' を作成できません"));
    assert!(!named("error: failed to push some refs"));
    // A remote's words, or a word in passing, are not git's refusal.
    assert!(!named(
        "remote: fatal: Unable to create '/srv/.git/index.lock': File exists."
    ));
    assert!(!named("a hook said index.lock"));
    assert_eq!(
        found(&lock),
        format!(
            "git found {}: another git may run, or a stop left it; M-x git-remove-stale-lock \
             removes it",
            shown()
        )
    );
    assert!(left(&lock).starts_with(&format!("the stop left {}: ", shown())));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn only_the_file_noted_is_taken_away() {
    let dir = dir("remove");
    let lock = lock(&dir);
    plant(&dir);
    // Another file in its place is left, and the note goes.
    #[cfg(unix)]
    {
        let noted = lock.after(None, true, false).unwrap();
        std::fs::remove_file(dir.join(".git/index.lock")).unwrap();
        std::fs::write(dir.join("hold"), "").unwrap();
        plant(&dir);
        let (said, kept) = lock.remove(noted.seen.id);
        assert!(said.contains("another file now"), "{said}");
        assert!(!kept);
        assert!(dir.join(".git/index.lock").exists());
    }
    // The file noted goes.
    let noted = lock.after(None, true, false).unwrap();
    let (said, kept) = lock.remove(noted.seen.id);
    assert_eq!(said, format!("Removed {}", shown()));
    assert!(!kept);
    assert!(!dir.join(".git/index.lock").exists());
    // Nothing there now.
    let (said, kept) = lock.remove(noted.seen.id);
    assert!(said.starts_with("No "), "{said}");
    assert!(!kept);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_link_is_the_lock_itself_and_its_target_stays() {
    let dir = dir("link");
    let lock = lock(&dir);
    let target = dir.join("target");
    std::fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, dir.join(".git/index.lock")).unwrap();
    let noted = lock.after(None, true, false).unwrap();
    let (said, _) = lock.remove(noted.seen.id);
    assert!(said.starts_with("Removed"), "{said}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_file_whose_inode_changed_since_is_not_the_one_noted() {
    use std::os::unix::fs::PermissionsExt;
    let dir = dir("changed");
    let lock = lock(&dir);
    plant(&dir);
    let noted = lock.after(None, true, false).unwrap();
    // Its device and inode stay, as a reused number would, but its change time moves on.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let path = dir.join(".git/index.lock");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (said, kept) = lock.remove(noted.seen.id);
    assert!(said.contains("another file now"), "{said}");
    assert!(!kept);
    assert!(path.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
