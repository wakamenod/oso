//! A status's thread and the index's lock (ADR 0127, "A stale index.lock, as amended"): staging
//! that fails on a lock there notes it and says so, and works again once it is gone.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use super::{Repo, Report, Section, Target, View, read, run, state};

#[test]
fn staging_that_fails_on_the_index_s_lock_notes_it_and_says_so() {
    let repo = Repo::new("vcs-lock");
    repo.write("base.txt", b"base\n");
    repo.commit("first");
    repo.write("new.txt", b"new\n");
    let mut state = state(&repo);
    let laid = read(&mut state, &repo.root, &View::default());
    std::fs::write(repo.root.join(".git/index.lock"), "").unwrap();
    let (_, reports) = run(
        &mut state,
        &repo.root,
        &laid,
        &[Target::Section(Section::Untracked)],
        true,
        &View::default(),
    );
    let [Report::Locked { noted, said }, ..] = reports.as_slice() else {
        panic!("the lock is noted: {reports:?}");
    };
    assert!(!noted.stopped);
    assert_eq!(said, &crate::vcs::lock::found(&noted.lock));
    // Once it is gone, staging works again.
    let (said, kept) = noted.lock.remove(noted.seen.id);
    assert!(said.starts_with("Removed"), "{said}");
    assert!(!kept);
    let laid = read(&mut state, &repo.root, &View::default());
    let (_, reports) = run(
        &mut state,
        &repo.root,
        &laid,
        &[Target::Section(Section::Untracked)],
        true,
        &View::default(),
    );
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
}
