//! A repository's run thread over stashes: a push with a message and untracked files, an apply
//! that keeps the stash, a drop that finds its stash again after another was pushed over it, one
//! gone refused, and a pop that does not apply cleanly keeping its stash.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use super::super::{RunEnd, RunRequest, StashRun};
use super::{repo, runner, until_end};
use crate::review::tests::Repo;
use crate::vcs::stash::Act;

fn text(repo: &Repo, args: &[&str]) -> String {
    String::from_utf8(repo.git(args)).unwrap().trim().to_owned()
}

fn push(message: &str, untracked: bool) -> RunRequest {
    RunRequest::Stash {
        run: StashRun::Push {
            untracked,
            message: message.to_owned(),
        },
        pass: Vec::new(),
    }
}

fn on(act: Act, id: &str) -> RunRequest {
    RunRequest::Stash {
        run: StashRun::On {
            act,
            id: id.to_owned(),
        },
        pass: Vec::new(),
    }
}

#[test]
fn a_push_takes_untracked_files_and_its_message_and_an_apply_keeps_it() {
    let repo = repo("stash-push");
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    std::fs::write(repo.root.join("new.txt"), "new\n").unwrap();
    let (runner, got) = runner(&repo);
    assert!(runner.ask(push("half done", true)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(
        output.contains("$ git stash push --include-untracked --message=half done"),
        "{output}"
    );
    assert!(!repo.root.join("new.txt").exists());
    assert_eq!(
        text(&repo, &["stash", "list", "--format=%gs"]),
        "On main: half done"
    );
    let id = text(&repo, &["rev-parse", "stash@{0}"]);
    assert!(runner.ask(on(Act::Apply, &id)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).unwrap(),
        "two\n"
    );
    assert_eq!(text(&repo, &["stash", "list", "--format=%H"]), id);
}

#[test]
fn a_drop_finds_its_stash_again_and_one_gone_is_refused() {
    let repo = repo("stash-drop");
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["stash", "push", "-q", "-m", "older"]);
    let older = text(&repo, &["rev-parse", "stash@{0}"]);
    // Another worktree's push moves it to stash@{1}.
    std::fs::write(repo.root.join("a.txt"), "three\n").unwrap();
    repo.git(&["stash", "push", "-q", "-m", "newer"]);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(on(Act::Drop, &older)));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(output.contains("$ git stash drop stash@{1}"), "{output}");
    assert_eq!(
        text(&repo, &["stash", "list", "--format=%gs"]),
        "On main: newer"
    );
    assert!(runner.ask(on(Act::Pop, &older)));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("gone")),
        "{end:?}"
    );
}

#[test]
fn a_pop_that_does_not_apply_cleanly_keeps_its_stash() {
    let repo = repo("stash-conflict");
    std::fs::write(repo.root.join("a.txt"), "stashed\n").unwrap();
    repo.git(&["stash", "push", "-q"]);
    let id = text(&repo, &["rev-parse", "stash@{0}"]);
    std::fs::write(repo.root.join("a.txt"), "committed\n").unwrap();
    repo.git(&["commit", "-q", "-am", "again"]);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(on(Act::Pop, &id)));
    let (end, output, _) = until_end(&got);
    let RunEnd::Failed { said, .. } = end else {
        panic!("{end:?}: {output}");
    };
    assert!(said.contains("the stash is kept"), "{said}");
    assert_eq!(text(&repo, &["stash", "list", "--format=%H"]), id);
}
