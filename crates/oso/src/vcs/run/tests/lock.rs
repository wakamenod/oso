//! The run thread and the index's lock (ADR 0168): a stop that
//! leaves it, as a hook stands in for, is noted before the end and taken away only after the
//! person's `y`, still the same file; a commit that fails on a lock says so, in a worktree too,
//! and names no hook.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use std::sync::mpsc;
use std::time::Duration;

use oso_core::editor::CommitSwitches;

use super::{commit, repo, runner};
use crate::vcs::lock::Noted;
use crate::vcs::run::{RunEnd, RunReport, RunRequest};

/// The lock noted before a run's end, if any, and the end.
fn noted_and_end(got: &mpsc::Receiver<RunReport>) -> (Option<Noted>, RunEnd) {
    let mut noted = None;
    loop {
        match got.recv_timeout(Duration::from_secs(30)).expect("a report") {
            RunReport::Locked(lock) => noted = Some(lock),
            RunReport::Ended(end) => return (noted, end),
            _ => {}
        }
    }
}

/// What taking the lock away said, and whether the note stays.
fn removed(got: &mpsc::Receiver<RunReport>) -> (String, bool) {
    loop {
        match got.recv_timeout(Duration::from_secs(30)).expect("a report") {
            RunReport::LockRemoved { said, kept } => return (said, kept),
            RunReport::Ended(end) => panic!("no run was asked for: {end:?}"),
            _ => {}
        }
    }
}

#[cfg(unix)]
#[test]
fn a_stop_that_leaves_the_lock_notes_it_and_y_takes_it_away() {
    let repo = repo("run-lock-stop");
    // A plain commit holds the lock only for the moments before its hook, where a stop leaves
    // it at times (ADR 0168); the hook makes one there itself,
    // as such a stop would leave it, and waits for the stop. Its mark says the lock is the
    // hook's: the one seen before it may still be git's own, which git takes away as it stops.
    let made = repo.dir.join("lock-made");
    let script = format!(
        "#!/bin/sh\n: > .git/index.lock\n: > '{}'\nexec sleep 60\n",
        made.display()
    );
    super::hook(&repo, "pre-commit", &script);
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let lock = repo.root.join(".git/index.lock");
    let (runner, got) = runner(&repo);
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let Ok(RunReport::Began { .. }) = got.recv_timeout(Duration::from_secs(30)) else {
        panic!("the run begins");
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !made.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the hook makes the lock"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    runner.stop();
    let (noted, end) = noted_and_end(&got);
    assert_eq!(end, RunEnd::Stopped);
    let noted = noted.expect("the stop left the lock");
    assert!(noted.stopped);
    assert_eq!(noted.lock.shown(), ".git/index.lock");
    assert!(lock.exists());

    // The next commit fails on it, and says so.
    std::fs::remove_file(repo.root.join(".git/hooks/pre-commit")).unwrap();
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let (found, end) = noted_and_end(&got);
    let found = found.expect("the failure found the lock");
    assert!(!found.stopped);
    assert_eq!(found.seen.id, noted.seen.id);
    assert_eq!(
        end,
        RunEnd::Failed {
            said: crate::vcs::lock::found(&noted.lock),
            hooks: Vec::new(),
        }
    );

    runner.ask(RunRequest::RemoveLock { id: noted.seen.id });
    assert_eq!(removed(&got), ("Removed .git/index.lock".to_owned(), false));
    assert!(!lock.exists());
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let (noted, end) = noted_and_end(&got);
    assert_eq!((noted, end), (None, RunEnd::Done));
    assert_eq!(super::subject(&repo), "Change a");
}

#[test]
fn a_commit_that_fails_on_a_lock_says_so_and_names_no_hook() {
    let repo = repo("run-lock-found");
    #[cfg(unix)]
    super::hook(&repo, "pre-commit", "#!/bin/sh\nexit 0\n");
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["add", "a.txt"]);
    std::fs::write(repo.root.join(".git/index.lock"), "").unwrap();
    let (runner, got) = runner(&repo);
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let (noted, end) = noted_and_end(&got);
    let noted = noted.expect("the failure found the lock");
    let RunEnd::Failed { said, hooks } = end else {
        panic!("the commit fails: {end:?}");
    };
    assert!(said.starts_with("git found "), "{said}");
    assert!(said.contains("M-x git-remove-stale-lock"), "{said}");
    assert!(hooks.is_empty());
    // Another file put in its place since is left.
    std::fs::remove_file(repo.root.join(".git/index.lock")).unwrap();
    std::fs::write(repo.dir.join("hold"), "").unwrap();
    std::fs::write(repo.root.join(".git/index.lock"), "").unwrap();
    runner.ask(RunRequest::RemoveLock { id: noted.seen.id });
    let (said, kept) = removed(&got);
    #[cfg(unix)]
    assert!(said.contains("another file now"), "{said}");
    assert!(!kept, "{said}");
    #[cfg(unix)]
    assert!(repo.root.join(".git/index.lock").exists());
}

#[test]
fn a_worktree_s_lock_is_in_its_own_git_directory() {
    let repo = repo("run-lock-worktree");
    let tree = repo.dir.join("tree");
    repo.git(&[
        "worktree", "add", "-q", "-b", "side",
        // Relative: git takes no Windows verbatim path (`\\?\C:\…`), as the scratch's is.
        "../tree",
    ]);
    let gitdir = repo.root.join(".git/worktrees/tree");
    std::fs::write(tree.join("a.txt"), "two\n").unwrap();
    repo.git(&["-C", "../tree", "add", "a.txt"]);
    std::fs::write(gitdir.join("index.lock"), "").unwrap();
    let (sent, got) = mpsc::channel();
    let runner = crate::vcs::run::Runner::start(
        crate::git::TrustedRoot::for_tests(tree.clone()),
        Some(repo.dir.join("state")),
        move |report| sent.send(report).is_ok(),
    )
    .unwrap();
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let (noted, end) = noted_and_end(&got);
    let noted = noted.expect("the failure found the worktree's lock");
    assert!(matches!(end, RunEnd::Failed { .. }), "{end:?}");
    let shown = std::path::PathBuf::from(noted.lock.shown());
    let shown = shown.canonicalize().unwrap();
    assert_eq!(shown, gitdir.join("index.lock").canonicalize().unwrap());
    // The main work tree's own lock was never there.
    assert!(!repo.root.join(".git/index.lock").exists());
}
