//! What an agent's session tool looks at before the person is asked (ADR 0121, step 6), over
//! temporary repositories: where a new branch starts, and what is refused before asking.
//!
//! Must not depend on: `app` or the jobs' threads; these call `prepare` directly.

use std::fs;
use std::path::Path;

use super::super::prepare::{Prepare, Prepared, prepare};
use super::Scratch;

fn worktree(branch: &str, base: Option<&str>, caller: &Path) -> Prepare {
    Prepare::Worktree {
        branch: branch.to_owned(),
        base: base.map(str::to_owned),
        caller: caller.to_owned(),
    }
}

/// Where a prepared worktree's new branch starts, if it is new.
fn start(scratch: &Scratch, branch: &str, caller: &Path) -> Option<(String, String)> {
    match prepare(&scratch.repo(), &worktree(branch, None, caller)).unwrap() {
        Prepared::Worktree { start, .. } => start,
        other @ Prepared::Directory(_) => panic!("{other:?}"),
    }
}

fn head_of(scratch: &Scratch, revision: &str) -> String {
    let out = scratch.git(&["rev-parse", revision]);
    String::from_utf8(out).unwrap().trim().to_owned()
}

#[test]
fn a_new_branch_starts_at_the_callers_head_and_an_existing_one_at_its_tip() {
    let scratch = Scratch::new("prepare-branch");
    let found = prepare(&scratch.repo(), &worktree("x", None, &scratch.root)).unwrap();
    let Prepared::Worktree { place, branch, .. } = found else {
        panic!("{found:?}");
    };
    assert_eq!(branch, "x");
    assert!(place.ends_with(".claude/worktrees/x"), "{place:?}");
    assert_eq!(
        start(&scratch, "x", &scratch.root),
        Some((head_of(&scratch, "HEAD"), "main".to_owned()))
    );
    // A worktree session on its own branch, whose commit a new branch then starts from.
    let made = scratch.create("work").unwrap();
    fs::write(made.path.join("b.txt"), "b\n").unwrap();
    scratch.git_in(&made.path, &["add", "."]);
    scratch.git_in(&made.path, &["commit", "-q", "-m", "work"]);
    assert_eq!(
        start(&scratch, "y", &made.path),
        Some((head_of(&scratch, "work"), "work".to_owned()))
    );
    // An existing branch takes no base, and starts at its tip.
    scratch.git(&["branch", "old"]);
    assert_eq!(start(&scratch, "old", &scratch.root), None);
    let why = prepare(
        &scratch.repo(),
        &worktree("old", Some("main"), &scratch.root),
    )
    .unwrap_err();
    assert!(why.contains("exists"), "{why}");
}

#[test]
fn what_cannot_happen_is_refused_before_anyone_is_asked() {
    let scratch = Scratch::new("prepare-refused");
    let repo = scratch.repo();
    let made = scratch.create("work").unwrap();
    let why = prepare(&repo, &worktree("work", None, &scratch.root)).unwrap_err();
    assert!(why.contains("checked out already"), "{why}");
    let why = prepare(&repo, &worktree("-x", None, &scratch.root)).unwrap_err();
    assert!(why.contains("Not a branch name"), "{why}");
    let why = prepare(&repo, &worktree("z", Some("nowhere"), &scratch.root)).unwrap_err();
    assert!(why.contains("Not a commit"), "{why}");
    let elsewhere = scratch.dir.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let why = prepare(&repo, &worktree("z", None, &elsewhere)).unwrap_err();
    assert!(why.contains("not this repository's"), "{why}");
    // A session's directory: the repository's root or one of its linked worktrees only.
    assert_eq!(
        prepare(&repo, &Prepare::Directory(scratch.root.clone())),
        Ok(Prepared::Directory(scratch.root.clone()))
    );
    assert_eq!(
        prepare(&repo, &Prepare::Directory(made.path.clone())),
        Ok(Prepared::Directory(made.path.clone()))
    );
    let why = prepare(&repo, &Prepare::Directory(elsewhere)).unwrap_err();
    assert!(why.contains("neither"), "{why}");
}
