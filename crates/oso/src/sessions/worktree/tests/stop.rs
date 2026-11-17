//! Stopping a worktree removal (ADR 0121, "Stopping a removal, as built"): before git runs,
//! nothing is removed, and a listing meanwhile marks the worktree; while git runs, it is
//! stopped at once.
//!
//! Must not depend on: `app`, `dispatch` or winit; these ask the jobs as the app does, and read
//! their reports.

use std::fs;
use std::time::{Duration, Instant};

use super::super::jobs::{
    Asked, Jobs, Phase, Report, STOPPED_BEFORE_GIT, STOPPED_IN_GIT, Snapshot, Stop,
};
use super::Scratch;
use super::jobs::{jobs, merged, next, slowed, trust};

/// Waits until no removal runs, failing past 10 s. A job reports its end before its thread
/// lets go of the removal's entry, so a loaded machine can see it a moment after the report.
fn none_run(jobs: &Jobs) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !jobs.running_removals().is_empty() {
        assert!(Instant::now() < deadline, "a removal still runs");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_removal_stopped_before_git_runs_removes_nothing() {
    let s = Scratch::new("stop-before");
    let [one] = merged(&s, ["one"]);
    let (jobs, got) = jobs(&s);
    jobs.remove(
        1,
        trust(&s.root, &[&s.root]),
        one.clone(),
        Snapshot::default(),
        Asked::Unasked,
    );
    let Report::Clear { go, .. } = next(&got) else {
        panic!("the app is asked to clear it");
    };
    let running = jobs.running_removals();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].phase, Phase::BeforeGit, "{running:?}");
    // A listing meanwhile marks it.
    jobs.list(2, trust(&s.root, &[&s.root]), Snapshot::default());
    loop {
        match next(&got) {
            Report::Row { looked, .. } => {
                assert!(looked.removing, "the row says it is being removed");
                break;
            }
            Report::Listed { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(jobs.stop_removal_before_git(1), Stop::BeforeGit);
    // The app clears nothing for a stopped removal: it drops its word.
    assert!(jobs.removal_stopped(1));
    drop(go);
    loop {
        match next(&got) {
            Report::Removed { result, .. } => {
                assert_eq!(result, Err(STOPPED_BEFORE_GIT.into()));
                break;
            }
            Report::ListEnded { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    assert!(one.join("a.txt").is_file(), "nothing was removed");
    none_run(&jobs);
    assert_eq!(jobs.stop_removal_confirmed(1), Stop::Ended, "it has ended");
}

#[test]
fn a_removal_stopped_while_git_runs_ends_at_once() {
    let s = Scratch::new("stop-in-git");
    let [one] = merged(&s, ["one"]);
    let slow = slowed(&s, &[&one], 8);
    let (jobs, got) = jobs(&s);
    jobs.remove(
        1,
        trust(&s.root, &[&s.root]),
        one.clone(),
        Snapshot::default(),
        Asked::Unasked,
    );
    match next(&got) {
        Report::Clear { go, .. } => {
            fs::write(&slow, "").unwrap();
            go.proceed();
        }
        other => panic!("{other:?}"),
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    // git runs, and waits in its filter.
    let running = s.dir.join("slow-running");
    while !running.exists() {
        assert!(Instant::now() < deadline, "git never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let stopped = Instant::now();
    // Once git runs, only a confirmed stop reaches it.
    assert_eq!(jobs.stop_removal_before_git(1), Stop::Ask);
    assert_eq!(jobs.stop_removal_confirmed(1), Stop::InGit);
    match next(&got) {
        Report::Removed { result, .. } => assert_eq!(result, Err(STOPPED_IN_GIT.into())),
        other => panic!("{other:?}"),
    }
    // `SIGTERM` ended git and its 8 s filter at once, not after the 5 s grace.
    assert!(
        stopped.elapsed() < Duration::from_secs(4),
        "{:?}",
        stopped.elapsed()
    );
    assert!(
        !s.git(&["branch", "--list", "one"]).is_empty(),
        "the branch is kept"
    );
}

#[test]
fn a_removal_waiting_for_a_thread_can_be_stopped_and_then_runs_nothing() {
    let s = Scratch::new("stop-queued");
    let [one, two, three] = merged(&s, ["one", "two", "three"]);
    let (jobs, got) = jobs(&s);
    let trust = || trust(&s.root, &[&s.root]);
    for (token, path) in [(1, &one), (2, &two)] {
        jobs.remove(
            token,
            trust(),
            path.clone(),
            Snapshot::default(),
            Asked::Unasked,
        );
    }
    // Both pool threads wait for the app's word.
    let mut goes = Vec::new();
    while goes.len() < 2 {
        match next(&got) {
            Report::Clear { go, .. } => goes.push(go),
            other => panic!("{other:?}"),
        }
    }
    jobs.remove(
        3,
        trust(),
        three.clone(),
        Snapshot::default(),
        Asked::Unasked,
    );
    let queued = jobs.running_removals();
    let waiting = queued.iter().find(|r| r.id == 3).expect("entered as asked");
    assert_eq!(waiting.resolved, None, "its job has not started");
    assert_eq!(jobs.stop_removal_before_git(3), Stop::BeforeGit);
    // The app calls the first two off; the third then gets its thread.
    drop(goes);
    let mut ended = Vec::new();
    while ended.len() < 3 {
        match next(&got) {
            Report::Removed { token, result, .. } => ended.push((token, result)),
            other => panic!("nothing else, no Clear for the stopped one: {other:?}"),
        }
    }
    let third = ended.iter().find(|(token, _)| *token == 3).unwrap();
    assert_eq!(third.1, Err(STOPPED_BEFORE_GIT.into()));
    assert!(three.join("a.txt").is_file(), "nothing was removed");
    none_run(&jobs);
}
