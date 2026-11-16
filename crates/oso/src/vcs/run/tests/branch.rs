//! A repository's run thread over branches: the list a prompt is given, checking out a local
//! branch and a remote's, making one at a start, renaming, a delete that is merged, one that
//! asks first and is refused once the branch moved, and one checked out in a worktree.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use std::sync::mpsc;
use std::time::Duration;

use super::super::{RunEnd, RunReport, RunRequest, Runner};
use super::remote::with_remote;
use super::{repo, runner, until_end};
use crate::review::tests::Repo;
use crate::vcs::branch::{BranchRead, BranchRun, Branches, DeletePlan};

fn ask(runner: &Runner, run: BranchRun) {
    assert!(runner.ask(RunRequest::Branch {
        run,
        pass: Vec::new(),
    }));
}

fn read(runner: &Runner, read: BranchRead) {
    assert!(runner.ask(RunRequest::BranchRead(read)));
}

fn text(repo: &Repo, args: &[&str]) -> String {
    String::from_utf8(repo.git(args)).unwrap().trim().to_owned()
}

/// Makes a change to `b.txt` with `text` and records it as `subject`.
fn change(repo: &Repo, text: &str, subject: &str) {
    std::fs::write(repo.root.join("b.txt"), text).unwrap();
    repo.git(&["add", "b.txt"]);
    repo.commit(subject);
}

/// The next list or plan the thread gives.
fn next(got: &mpsc::Receiver<RunReport>) -> RunReport {
    loop {
        let report = got.recv_timeout(Duration::from_secs(30)).unwrap();
        if matches!(
            report,
            RunReport::Branches(_) | RunReport::DeletePlanned { .. }
        ) {
            return report;
        }
    }
}

fn listed(runner: &Runner, got: &mpsc::Receiver<RunReport>) -> Branches {
    read(runner, BranchRead::List);
    match next(got) {
        RunReport::Branches(Ok(branches)) => branches,
        other => panic!("{other:?}"),
    }
}

fn planned(
    runner: &Runner,
    got: &mpsc::Receiver<RunReport>,
    name: &str,
) -> Result<DeletePlan, String> {
    read(runner, BranchRead::PlanDelete { name: name.into() });
    match next(got) {
        RunReport::DeletePlanned { plan, .. } => plan,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_list_names_local_and_remote_branches_and_checking_out_a_remote_s_tracks_it() {
    let (repo, _) = with_remote("branch-list");
    repo.git(&["push", "-q", "origin", "main:theirs"]);
    repo.git(&["fetch", "-q", "origin"]);
    repo.git(&["branch", "side"]);
    let (runner, got) = runner(&repo);
    let branches = listed(&runner, &got);
    assert_eq!(branches.local, ["main", "side"]);
    assert_eq!(branches.remote, ["origin/theirs"]);
    assert_eq!(branches.current.as_deref(), Some("main"));
    ask(
        &runner,
        BranchRun::Checkout {
            name: "origin/theirs".into(),
            remote: true,
        },
    );
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(
        output.contains("$ git switch --track refs/remotes/origin/theirs"),
        "{output}"
    );
    assert_eq!(
        text(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "theirs"
    );
    let upstream = text(&repo, &["rev-parse", "--abbrev-ref", "theirs@{upstream}"]);
    assert_eq!(upstream, "origin/theirs");
    ask(
        &runner,
        BranchRun::Checkout {
            name: "side".into(),
            remote: false,
        },
    );
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(text(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "side");
}

#[test]
fn a_branch_is_made_at_its_start_and_renamed() {
    let repo = repo("branch-create");
    change(&repo, "b\n", "second");
    let first = text(&repo, &["rev-parse", "HEAD~"]);
    let (runner, got) = runner(&repo);
    ask(
        &runner,
        BranchRun::Create {
            name: "old".into(),
            start: "HEAD~".into(),
            checkout: false,
        },
    );
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(text(&repo, &["rev-parse", "old"]), first);
    assert_eq!(text(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    ask(
        &runner,
        BranchRun::Create {
            name: "-x".into(),
            start: "HEAD".into(),
            checkout: true,
        },
    );
    let (end, _, _) = until_end(&got);
    assert!(matches!(end, RunEnd::Refused(_)), "{end:?}");
    ask(
        &runner,
        BranchRun::Rename {
            old: "old".into(),
            new: "new".into(),
        },
    );
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(text(&repo, &["rev-parse", "new"]), first);
}

#[test]
fn a_merged_branch_deletes_and_one_not_merged_asks_and_is_refused_once_moved() {
    let repo = repo("branch-delete");
    repo.git(&["branch", "merged"]);
    repo.git(&["checkout", "-q", "-b", "own"]);
    change(&repo, "b\n", "only here");
    repo.git(&["checkout", "-q", "main"]);
    let (runner, got) = runner(&repo);
    assert_eq!(planned(&runner, &got, "merged"), Ok(DeletePlan::Merged));
    let Ok(DeletePlan::Unmerged { tip, dropped }) = planned(&runner, &got, "own") else {
        panic!("not merged");
    };
    let (named, count) = dropped.expect("git counted them");
    assert_eq!(count, 1);
    assert!(named[0].ends_with(" only here"), "{named:?}");
    // It moves before the `y`: the forced delete is refused.
    repo.git(&["checkout", "-q", "own"]);
    change(&repo, "c\n", "more");
    repo.git(&["checkout", "-q", "main"]);
    let delete = |tip| BranchRun::Delete {
        name: "own".into(),
        tip: Some(tip),
    };
    ask(&runner, delete(tip));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("moved")),
        "{end:?}"
    );
    let Ok(DeletePlan::Unmerged { tip, dropped }) = planned(&runner, &got, "own") else {
        panic!("not merged");
    };
    assert_eq!(dropped.map(|(_, count)| count), Some(2));
    ask(&runner, delete(tip));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(output.contains("$ git branch -D own"), "{output}");
    // The one checked out here is refused.
    let refused = planned(&runner, &got, "main").unwrap_err();
    assert!(refused.contains("checked out here"), "{refused}");
}

#[test]
fn a_branch_checked_out_in_a_worktree_is_not_deleted() {
    let repo = repo("branch-worktree");
    let other = crate::git::for_git(&repo.dir.join("wt"));
    repo.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "side",
        other.to_str().unwrap(),
    ]);
    let (runner, got) = runner(&repo);
    let refused = planned(&runner, &got, "side").unwrap_err();
    assert!(refused.contains("worktree-remove"), "{refused}");
}

#[test]
fn a_list_past_what_a_prompt_takes_keeps_the_local_branches_and_5000_remote_ones() {
    let repo = repo("branch-many");
    let head = text(&repo, &["rev-parse", "HEAD"]);
    // Over 1 MiB of names, packed as git keeps them.
    let mut packed = String::from("# pack-refs with: peeled fully-peeled sorted \n");
    for n in 0..30_000 {
        packed.push_str(&format!(
            "{head} refs/remotes/origin/a-rather-long-branch-name-{n:06}\n"
        ));
    }
    std::fs::write(repo.root.join(".git/packed-refs"), packed).unwrap();
    let (runner, got) = runner(&repo);
    let branches = listed(&runner, &got);
    assert_eq!(branches.local, ["main"]);
    assert_eq!(branches.remote.len(), 5000);
}
