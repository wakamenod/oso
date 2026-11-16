//! A repository's run thread over what is stopped: a pull that rebases onto a conflict and says
//! so, aborting it, continuing it once resolved, a pull that merges past `pull.ff=only`, a
//! cherry-pick skipped, a merge that cannot skip and continues with git's message opening no
//! editor of the configuration's, a step refused when nothing, or something else, is stopped,
//! and a link in the git directory not followed.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use oso_core::editor::PullHow;

use super::super::{RemoteRun, RunEnd, RunRequest};
use super::remote::with_remote;
use super::{repo, runner, subject, until_end};
use crate::git::Git;
use crate::review::tests::Repo;
use crate::vcs::read::{git_dir, stopped as read_stopped};
use crate::vcs::sequence::{Doing, Step, Stop, Want};

/// What is stopped in `repo`, read as the status reads it.
fn stopped(repo: &Repo) -> Option<Stop> {
    let hooks = repo.dir.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let git = Git::new(&repo.root, &hooks).unwrap();
    read_stopped(&git, &git_dir(&git)?)
}

fn step(step: Step, stop: &Stop) -> RunRequest {
    RunRequest::Step {
        want: Want::new(step, stop),
        pass: Vec::new(),
    }
}

/// `doing` with nothing known of it, as a step asked when nothing is stopped.
fn blank(doing: Doing) -> Stop {
    Stop {
        doing,
        branch: None,
        onto: None,
        at: None,
        progress: None,
        orig: None,
        commits: Vec::new(),
    }
}

fn pull(how: PullHow) -> RunRequest {
    RunRequest::Remote(RemoteRun::Pull {
        how,
        pass: Vec::new(),
    })
}

/// A repository whose `main` and its upstream's changed `a.txt` apart: `mine` here, `theirs`
/// there, fetched or not.
fn gone_apart(name: &str, conflict: bool) -> Repo {
    let (repo, bare) = with_remote(name);
    repo.git(&["push", "-q", "-u", "origin", "main"]);
    let other = crate::git::for_git(&repo.dir.join("other"));
    let other = other.to_str().unwrap();
    repo.git(&["clone", "-q", &bare, other]);
    std::fs::write(repo.dir.join("other/a.txt"), "theirs\n").unwrap();
    repo.git(&["-C", other, "commit", "-q", "-am", "theirs"]);
    repo.git(&["-C", other, "push", "-q", "origin", "main"]);
    let (file, text) = if conflict {
        ("a.txt", "mine\n")
    } else {
        ("b.txt", "b\n")
    };
    std::fs::write(repo.root.join(file), text).unwrap();
    repo.git(&["add", file]);
    repo.git(&["commit", "-q", "-m", "mine"]);
    repo
}

#[test]
fn a_rebasing_pull_stops_on_a_conflict_and_an_abort_takes_it_back() {
    let repo = gone_apart("stop-pull-abort", true);
    let mine = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    let (runner, got) = runner(&repo);
    assert!(runner.ask(pull(PullHow::Rebase)));
    let (end, output, _) = until_end(&got);
    assert!(
        output.contains("$ git pull --rebase --progress"),
        "{output}"
    );
    let RunEnd::Failed { said, .. } = end else {
        panic!("{end:?}: {output}");
    };
    assert!(said.contains("stopped on a conflict"), "{said}");
    let stop = stopped(&repo).expect("a rebase stopped");
    assert_eq!(stop.doing, Doing::Rebase);
    assert_eq!(stop.branch.as_deref(), Some(&b"main"[..]));
    assert_eq!(stop.progress, Some((1, 1)));
    let named = stop.at.as_deref().map(|at| stop.named(at));
    assert_eq!(named.and_then(|(_, subject)| subject), Some(&b"mine"[..]));
    // An abort whose question named another commit is refused.
    let other = Stop {
        at: Some("d".repeat(40)),
        ..stop.clone()
    };
    assert!(runner.ask(step(Step::Abort, &other)));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("moved on")),
        "{end:?}"
    );
    assert!(runner.ask(step(Step::Abort, &stop)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(stopped(&repo), None);
    assert_eq!(
        String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap(),
        mine
    );
}

#[test]
fn a_rebase_continues_once_resolved_keeping_its_message() {
    let repo = gone_apart("stop-pull-continue", true);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(pull(PullHow::Rebase)));
    let _ = until_end(&got);
    let stop = stopped(&repo).expect("a rebase stopped");
    assert_eq!(stop.doing, Doing::Rebase);
    std::fs::write(repo.root.join("a.txt"), "both\n").unwrap();
    repo.git(&["add", "a.txt"]);
    assert!(runner.ask(step(Step::Continue, &stop)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(stopped(&repo), None);
    assert_eq!(subject(&repo), "mine");
    let parent = String::from_utf8(repo.git(&["log", "-1", "--format=%s", "HEAD~"])).unwrap();
    assert_eq!(parent.trim(), "theirs");
}

#[test]
fn a_merging_pull_makes_a_merge_commit_past_pull_ff_only() {
    let repo = gone_apart("stop-pull-merge", false);
    repo.git(&["config", "pull.ff", "only"]);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(pull(PullHow::Merge)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(
        output.contains("$ git pull --no-rebase --ff --no-edit --progress"),
        "{output}"
    );
    let parents = String::from_utf8(repo.git(&["log", "-1", "--format=%P"])).unwrap();
    assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
}

#[test]
fn a_picked_commit_that_conflicts_is_skipped() {
    let repo = repo("stop-pick");
    repo.git(&["checkout", "-q", "-b", "side"]);
    std::fs::write(repo.root.join("a.txt"), "side\n").unwrap();
    repo.git(&["commit", "-q", "-am", "side"]);
    let side = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    repo.git(&["checkout", "-q", "main"]);
    std::fs::write(repo.root.join("a.txt"), "main\n").unwrap();
    repo.git(&["commit", "-q", "-am", "main"]);
    assert!(!repo.git_ran(&["cherry-pick", side.trim()]).0);
    let stop = stopped(&repo).expect("a pick stopped");
    assert_eq!(stop.doing, Doing::CherryPick);
    assert_eq!(stop.at.as_deref(), Some(side.trim()));
    let (runner, got) = runner(&repo);
    assert!(runner.ask(step(Step::Skip, &stop)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(stopped(&repo), None);
    assert_eq!(subject(&repo), "main");
}

#[test]
fn a_merge_cannot_skip_and_continues_with_git_s_message() {
    let repo = repo("stop-merge");
    repo.git(&["checkout", "-q", "-b", "side"]);
    std::fs::write(repo.root.join("a.txt"), "side\n").unwrap();
    repo.git(&["commit", "-q", "-am", "side"]);
    repo.git(&["checkout", "-q", "main"]);
    std::fs::write(repo.root.join("a.txt"), "main\n").unwrap();
    repo.git(&["commit", "-q", "-am", "main"]);
    assert!(!repo.git_ran(&["merge", "-q", "side"]).0);
    let stop = stopped(&repo).expect("a merge stopped");
    assert_eq!(stop.doing, Doing::Merge);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(step(Step::Skip, &stop)));
    let (end, _, _) = until_end(&got);
    assert!(matches!(end, RunEnd::Refused(_)), "{end:?}");
    std::fs::write(repo.root.join("a.txt"), "both\n").unwrap();
    repo.git(&["add", "a.txt"]);
    // No editor of the configuration's opens: it would leave its mark.
    let mark = crate::git::for_git(&repo.dir.join("edited"));
    let editor = format!("touch '{}'", mark.to_str().unwrap());
    repo.git(&["config", "core.editor", &editor]);
    repo.git(&["config", "sequence.editor", &editor]);
    assert!(runner.ask(step(Step::Continue, &stop)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(subject(&repo), "Merge branch 'side'");
    assert!(!mark.exists());
}

#[test]
fn a_step_is_refused_when_nothing_or_something_else_is_stopped() {
    let repo = repo("stop-none");
    let (runner, got) = runner(&repo);
    assert!(runner.ask(step(Step::Continue, &blank(Doing::Rebase))));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("No rebase")),
        "{end:?}"
    );
    std::fs::write(repo.root.join(".git/MERGE_HEAD"), "").unwrap();
    assert!(runner.ask(step(Step::Abort, &blank(Doing::Rebase))));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("merge is stopped")),
        "{end:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_link_in_the_git_directory_is_not_followed() {
    let repo = repo("stop-link");
    let head = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    let elsewhere = repo.dir.join("elsewhere");
    std::fs::write(&elsewhere, head).unwrap();
    std::os::unix::fs::symlink(&elsewhere, repo.root.join(".git/MERGE_HEAD")).unwrap();
    assert_eq!(stopped(&repo), None);
}
