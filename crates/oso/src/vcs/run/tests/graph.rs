//! A repository's run thread over a graph's runs: a tag, plain and annotated, two commits
//! picked oldest first, a revert, a detached checkout, a pick that conflicts saying so, a
//! revert, a checkout and a branch's checkout refused while something is stopped, and ids that
//! are not whole refused before git runs.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use super::super::{RunEnd, RunRequest, Runner};
use super::{repo, runner, subject, until_end};
use crate::review::tests::Repo;
use crate::vcs::branch::BranchRun;
use crate::vcs::graph::GraphRun;

fn text(repo: &Repo, args: &[&str]) -> String {
    String::from_utf8(repo.git(args)).unwrap().trim().to_owned()
}

/// Makes a change to `file` with `text` and records it as `subject`.
fn change(repo: &Repo, file: &str, text: &str, subject: &str) {
    std::fs::write(repo.root.join(file), text).unwrap();
    repo.git(&["add", file]);
    repo.commit(subject);
}

fn graph(runner: &Runner, run: GraphRun) {
    assert!(runner.ask(RunRequest::Graph {
        run,
        pass: Vec::new(),
    }));
}

#[test]
fn a_tag_is_made_plain_or_annotated_at_its_commit() {
    let repo = repo("graph-tag");
    let first = text(&repo, &["rev-parse", "HEAD"]);
    change(&repo, "b.txt", "b\n", "second");
    let (runner, got) = runner(&repo);
    graph(
        &runner,
        GraphRun::Tag {
            name: "v1".into(),
            at: first.clone(),
            message: None,
        },
    );
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(text(&repo, &["rev-parse", "v1"]), first);
    graph(
        &runner,
        GraphRun::Tag {
            name: "v2".into(),
            at: first.clone(),
            message: Some("The first".into()),
        },
    );
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(
        text(&repo, &["tag", "-l", "--format=%(contents)", "v2"]),
        "The first"
    );
    graph(
        &runner,
        GraphRun::Tag {
            name: "-x".into(),
            at: first,
            message: None,
        },
    );
    let (end, _, _) = until_end(&got);
    assert!(matches!(end, RunEnd::Refused(_)), "{end:?}");
}

#[test]
fn picked_commits_go_on_oldest_first_and_a_revert_undoes_one() {
    let repo = repo("graph-pick");
    repo.git(&["checkout", "-q", "-b", "side"]);
    change(&repo, "b.txt", "b\n", "side one");
    let one = text(&repo, &["rev-parse", "HEAD"]);
    change(&repo, "c.txt", "c\n", "side two");
    let two = text(&repo, &["rev-parse", "HEAD"]);
    repo.git(&["checkout", "-q", "main"]);
    let (runner, got) = runner(&repo);
    graph(
        &runner,
        GraphRun::Pick {
            ids: vec![one, two],
        },
    );
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(subject(&repo), "side two");
    let parent = text(&repo, &["log", "-1", "--format=%s", "HEAD~"]);
    assert_eq!(parent, "side one");
    let head = text(&repo, &["rev-parse", "HEAD"]);
    graph(&runner, GraphRun::Revert { id: head });
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(subject(&repo), "Revert \"side two\"");
    assert!(!repo.root.join("c.txt").exists());
}

#[test]
fn a_pick_that_conflicts_says_so_and_then_picks_and_checkouts_are_refused() {
    let repo = repo("graph-conflict");
    repo.git(&["checkout", "-q", "-b", "side"]);
    change(&repo, "a.txt", "side\n", "side");
    let side = text(&repo, &["rev-parse", "HEAD"]);
    repo.git(&["checkout", "-q", "main"]);
    change(&repo, "a.txt", "main\n", "main");
    let main = text(&repo, &["rev-parse", "HEAD"]);
    let (runner, got) = runner(&repo);
    graph(
        &runner,
        GraphRun::Pick {
            ids: vec![side.clone()],
        },
    );
    let (end, output, _) = until_end(&got);
    let RunEnd::Failed { said, .. } = end else {
        panic!("{end:?}: {output}");
    };
    assert!(said.contains("stopped on a conflict"), "{said}");
    graph(&runner, GraphRun::Revert { id: main.clone() });
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("is stopped")),
        "{end:?}"
    );
    assert!(runner.ask(RunRequest::Branch {
        run: BranchRun::Detach { id: main },
        pass: Vec::new(),
    }));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("is stopped")),
        "{end:?}"
    );
}

#[test]
fn a_commit_is_checked_out_detached() {
    let repo = repo("graph-detach");
    let first = text(&repo, &["rev-parse", "HEAD"]);
    change(&repo, "b.txt", "b\n", "second");
    let (runner, got) = runner(&repo);
    assert!(runner.ask(RunRequest::Branch {
        run: BranchRun::Detach { id: first.clone() },
        pass: Vec::new(),
    }));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(text(&repo, &["rev-parse", "HEAD"]), first);
    assert_eq!(text(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
}

#[test]
fn ids_that_are_not_whole_are_refused_before_git_runs() {
    let repo = repo("graph-ids");
    let (runner, got) = runner(&repo);
    let short = "a".repeat(39);
    let runs = [
        RunRequest::Graph {
            run: GraphRun::Pick {
                ids: vec!["--no-commit".into()],
            },
            pass: Vec::new(),
        },
        RunRequest::Graph {
            run: GraphRun::Revert { id: "HEAD".into() },
            pass: Vec::new(),
        },
        RunRequest::Graph {
            run: GraphRun::Revert { id: short },
            pass: Vec::new(),
        },
        RunRequest::Graph {
            run: GraphRun::Tag {
                name: "v1".into(),
                at: "--points-at".into(),
                message: None,
            },
            pass: Vec::new(),
        },
        RunRequest::Branch {
            run: BranchRun::Detach { id: "-".into() },
            pass: Vec::new(),
        },
    ];
    for run in runs {
        assert!(runner.ask(run));
        let (end, output, began) = until_end(&got);
        assert!(
            matches!(&end, RunEnd::Refused(why) if why == "Not a commit's id"),
            "{end:?}"
        );
        assert!(!began, "{output}");
    }
}

#[test]
fn a_branch_s_checkout_is_refused_while_a_pick_is_stopped() {
    let repo = repo("graph-checkout-stopped");
    repo.git(&["branch", "other"]);
    repo.git(&["checkout", "-q", "-b", "side"]);
    change(&repo, "a.txt", "side\n", "side");
    let side = text(&repo, &["rev-parse", "HEAD"]);
    repo.git(&["checkout", "-q", "main"]);
    change(&repo, "a.txt", "main\n", "main");
    assert!(!repo.git_ran(&["cherry-pick", &side]).0);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(RunRequest::Branch {
        run: BranchRun::Checkout {
            name: "other".into(),
            remote: false,
        },
        pass: Vec::new(),
    }));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("is stopped")),
        "{end:?}"
    );
}
