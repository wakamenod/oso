//! Worktree jobs on the pool over temporary repositories: a root no longer trusted, one put in
//! its place, a worktree whose main repository is not trusted, and a removal that runs every
//! check again and waits for the app's word.
//!
//! Must not depend on: `app`, `dispatch` or winit; these ask the jobs as the app does, and read
//! their reports.

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::super::jobs::{Jobs, MAIN_UNTRUSTED, Report, Snapshot, Trust};
use super::{Scratch, order};
use crate::review::TrustedRoot;

/// Jobs keeping their state under the scratch directory, and their reports.
fn jobs(s: &Scratch) -> (Jobs, mpsc::Receiver<Report>) {
    let (sent, got) = mpsc::channel();
    let jobs = Jobs::new(Some(s.dir.join("job-state")), move |report| {
        let _ = sent.send(report);
    });
    (jobs, got)
}

/// The trust of `root`, with `trusted` the roots the record holds.
fn trust(root: &Path, trusted: &[&Path]) -> Trust {
    Trust {
        root: TrustedRoot::for_tests(root.to_owned()),
        trusted: trusted.iter().map(|p| p.to_path_buf()).collect(),
    }
}

/// The next report, waiting at most 20 s.
fn next(got: &mpsc::Receiver<Report>) -> Report {
    got.recv_timeout(Duration::from_secs(20)).expect("a report")
}

#[test]
fn a_revoked_root_runs_no_job() {
    let s = Scratch::new("revoked-job");
    let (jobs, got) = jobs(&s);
    let trust = trust(&s.root, &[&s.root]);
    trust.root.revoke();
    jobs.create(1, trust, order("never"));
    match next(&got) {
        Report::Created {
            result: Err(why), ..
        } => assert!(why.contains("no longer trusted"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(!s.root.join(".claude").exists());
    assert!(s.git(&["branch", "--list", "never"]).is_empty());
}

#[test]
fn a_root_put_in_the_trusted_ones_place_runs_no_job() {
    let s = Scratch::new("replaced-job");
    let trust = trust(&s.root, &[&s.root]);
    fs::rename(&s.root, s.dir.join("moved")).unwrap();
    fs::create_dir(&s.root).unwrap();
    let (jobs, got) = jobs(&s);
    jobs.list(2, trust, Snapshot::default());
    match next(&got) {
        Report::Listed {
            result: Err(why), ..
        } => assert!(why.contains("no longer the directory"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_worktree_whose_main_repository_is_not_trusted_makes_nothing() {
    let s = Scratch::new("main-untrusted");
    let linked = s.create("linked").unwrap().path;
    let (jobs, got) = jobs(&s);
    // The linked worktree is trusted on its own; its main repository is not.
    jobs.create(3, trust(&linked, &[&linked]), order("next"));
    match next(&got) {
        Report::Created {
            result: Err(why), ..
        } => assert_eq!(why, MAIN_UNTRUSTED),
        other => panic!("{other:?}"),
    }
    assert!(!s.root.join(".claude/worktrees/next").exists());
    assert!(s.git(&["branch", "--list", "next"]).is_empty());
    jobs.create(4, trust(&linked, &[&linked, &s.root]), order("next"));
    loop {
        match next(&got) {
            Report::Step { .. } => {}
            Report::Created { result, .. } => {
                assert!(result.is_ok(), "{result:?}");
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(s.root.join(".claude/worktrees/next").is_dir());
}

#[test]
fn a_removal_runs_every_check_again() {
    let s = Scratch::new("recheck");
    s.with_origin();
    let made = s.create("done").unwrap();
    fs::write(made.path.join("b.txt"), "b\n").unwrap();
    s.git_in(&made.path, &["add", "b.txt"]);
    s.git_in(&made.path, &["commit", "-q", "-m", "work"]);
    s.git(&["merge", "-q", "done"]);
    s.git(&["push", "-q", "origin", "main"]);
    let (jobs, got) = jobs(&s);
    let trust = || trust(&s.root, &[&s.root]);
    let removed = |token| match next(&got) {
        Report::Removed {
            token: t, result, ..
        } if t == token => result,
        other => panic!("{other:?}"),
    };
    jobs.check(1, trust(), made.path.clone(), Snapshot::default());
    match next(&got) {
        Report::Checked {
            result: Ok(ignored),
            ..
        } => assert!(ignored.is_empty(), "{ignored:?}"),
        other => panic!("{other:?}"),
    }
    // A file made after the question.
    fs::write(made.path.join("new.txt"), "n").unwrap();
    jobs.remove(2, trust(), made.path.clone(), Snapshot::default(), 0);
    let why = removed(2).unwrap_err();
    assert!(why.contains("untracked"), "{why}");
    fs::remove_file(made.path.join("new.txt")).unwrap();
    // A file git ignores, which the question did not name.
    fs::write(made.path.join(".env"), "x").unwrap();
    jobs.remove(3, trust(), made.path.clone(), Snapshot::default(), 0);
    let why = removed(3).unwrap_err();
    assert!(why.contains("ignores more"), "{why}");
    assert!(made.path.is_dir(), "nothing was removed");
    // The checks hold: the app is asked to clear it, and calls it off.
    jobs.remove(4, trust(), made.path.clone(), Snapshot::default(), 1);
    match next(&got) {
        Report::Clear { token: 4, go, .. } => drop(go),
        other => panic!("{other:?}"),
    }
    assert!(removed(4).unwrap_err().contains("called off"));
    assert!(made.path.is_dir(), "nothing was removed");
    jobs.remove(5, trust(), made.path.clone(), Snapshot::default(), 1);
    match next(&got) {
        Report::Clear { token: 5, path, go } => {
            assert_eq!(path, made.path);
            go.proceed();
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(removed(5), Ok(()));
    assert!(!made.path.exists());
}
