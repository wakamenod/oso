//! Tests of moving a session that read a registry and the process table (ADR 0151): which
//! entry is the asker's, the repository it is placed by, and the wait's thread. Unix only:
//! elsewhere the system says no start time, and a move is refused before any of this runs.
//!
//! Must not depend on: `app`, or the system's own registry; these use a scratch directory of
//! their own.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::super::{Look, NOT_FOUND, POLL, look, spawn_wait};
use super::{ID, PID, started, still};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("oso-moving-{name}-{}-{at}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.0.join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A main checkout at `name`: a directory with a `.git` directory.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.dir(name);
        fs::create_dir_all(repo.join(".git")).unwrap();
        repo
    }

    /// The registry, holding an entry of [`PID`] that ran in `cwd` from `started`.
    fn registry(&self, cwd: &str, started: SystemTime) -> PathBuf {
        self.registry_of(cwd, started, "interactive")
    }

    /// The registry, holding an entry of [`PID`] of `kind` that ran in `cwd` from `started`.
    fn registry_of(&self, cwd: &str, started: SystemTime, kind: &str) -> PathBuf {
        let dir = self.dir("registry");
        let ms = started.duration_since(UNIX_EPOCH).unwrap().as_millis();
        let entry = format!(
            r#"{{"pid":{PID},"sessionId":"{ID}","cwd":"{cwd}","startedAt":{ms},"name":"api-fix","kind":"{kind}","status":"busy"}}"#
        );
        fs::write(dir.join(format!("{PID}.json")), entry).unwrap();
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The look of [`PID`] started at [`started`], the home directory `home`, `trusted` trusted.
fn asked(home: &Path, trusted: &[PathBuf]) -> Look {
    Look {
        pid: PID,
        started: started(),
        program: Some("claude".into()),
        trusted: Arc::from(trusted),
        home: Some(home.to_path_buf()),
    }
}

#[test]
fn the_asker_is_the_bridges_parent_by_its_registry_entry() {
    let scratch = Scratch::new("found");
    let repo = scratch.repo("api");
    let cwd = scratch.dir("api/src");
    let registry = scratch.registry(&cwd.display().to_string(), started());
    let asker = look(Some(&registry), &asked(&scratch.0, &[]), still).unwrap();
    assert_eq!(asker.id.as_str(), ID);
    assert_eq!(asker.pid, PID);
    assert_eq!(asker.name.as_deref(), Some("api-fix"));
    assert_eq!(asker.directory, cwd);
    assert_eq!(asker.program.as_deref(), Some("claude"));
    assert_eq!(asker.main.root, repo);
    assert!(asker.main.identity.is_some());
    assert_eq!(asker.worktree, None);
}

#[test]
fn an_entry_that_is_not_the_parents_as_it_connected_is_refused() {
    let scratch = Scratch::new("other");
    scratch.repo("api");
    let cwd = scratch.0.join("api").display().to_string();
    let home = scratch.0.clone();
    // No registry, or none with the parent's entry.
    assert_eq!(
        look(None, &asked(&home, &[]), still).unwrap_err(),
        NOT_FOUND
    );
    let empty = scratch.dir("empty");
    assert_eq!(
        look(Some(&empty), &asked(&home, &[]), still).unwrap_err(),
        NOT_FOUND
    );
    // An entry from another process that had the id before.
    let registry = scratch.registry(&cwd, started() - Duration::from_secs(3600));
    assert_eq!(
        look(Some(&registry), &asked(&home, &[]), still).unwrap_err(),
        NOT_FOUND
    );
    // A run with `-p` does not move.
    let printing = scratch.registry_of(&cwd, started(), "print");
    let why = look(Some(&printing), &asked(&home, &[]), still).unwrap_err();
    assert!(why.contains("interactive"), "{why}");
    // The parent ended, and another process has its id now.
    let registry = scratch.registry(&cwd, started());
    let another = |_| Ok(Some(started() + Duration::from_secs(60)));
    let why = look(Some(&registry), &asked(&home, &[]), another).unwrap_err();
    assert!(why.contains("not the one that connected"), "{why}");
    let gone = |_| Ok(None);
    assert!(look(Some(&registry), &asked(&home, &[]), gone).is_err());
}

#[test]
fn a_directory_oso_cannot_place_is_refused() {
    let scratch = Scratch::new("places");
    let repo = scratch.repo("api");
    let home = scratch.0.clone();
    let refused = |cwd: &str| {
        let registry = scratch.registry(cwd, started());
        look(Some(&registry), &asked(&home, &[]), still).unwrap_err()
    };
    // The registry's reader keeps no directory it could not place a session by.
    assert_eq!(refused("api"), NOT_FOUND);
    let dotted = format!("{}/../api", repo.display());
    assert_eq!(refused(&dotted), NOT_FOUND);
    assert!(refused(&format!("{}/gone", repo.display())).contains("gone"));
    let plain = scratch.dir("notes");
    assert!(refused(&plain.display().to_string()).contains("no repository"));
    #[cfg(unix)]
    {
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&repo, &link).unwrap();
        assert!(refused(&link.display().to_string()).contains("link"));
    }
}

/// A worktree at `worktree` of the main repository at `main`, as `git worktree add` leaves
/// them.
fn worktree_pair(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let main = scratch.repo("main");
    let worktree = scratch.dir("wt");
    let entry = main.join(".git/worktrees/wt");
    fs::create_dir_all(&entry).unwrap();
    fs::write(
        entry.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", entry.display()),
    )
    .unwrap();
    (main, worktree)
}

#[test]
fn a_worktree_counts_only_with_its_main_repository_trusted() {
    let scratch = Scratch::new("worktree");
    let (main, worktree) = worktree_pair(&scratch);
    let registry = scratch.registry(&worktree.display().to_string(), started());
    let home = scratch.0.clone();
    let why = look(Some(&registry), &asked(&home, &[]), still).unwrap_err();
    assert!(why.contains("does not trust"), "{why}");
    let trusted = std::slice::from_ref(&main);
    let asker = look(Some(&registry), &asked(&home, trusted), still).unwrap();
    assert_eq!(asker.main.root, main);
    assert_eq!(asker.worktree, Some(worktree));
}

#[test]
fn the_wait_ends_when_told_without_a_step() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    // This test's own process runs all along, so only `stop` ends the wait.
    let pid = std::process::id();
    let then = oso_platform::process::start_time(pid).unwrap().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (sent, steps) = mpsc::channel();
    let (ended, done) = mpsc::channel::<()>();
    spawn_wait(pid, then, Arc::clone(&stop), move |step| {
        let _ = &ended;
        sent.send(step).is_ok()
    })
    .unwrap();
    stop.store(true, Ordering::Release);
    // The thread drops `send`, and with it `ended`, within a poll.
    let gone = done.recv_timeout(POLL * 4);
    assert_eq!(gone, Err(mpsc::RecvTimeoutError::Disconnected));
    assert!(steps.try_recv().is_err());
}
