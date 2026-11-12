//! Reviews of a repository's history as it can be: a merge commit, a shallow clone's oldest
//! commit, and a merge that left conflicts.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the review's reader directly.

use super::super::{Content, Report, Request, Scope};
use super::{Repo, Status, added, ask, open};

#[test]
fn a_merge_commit_is_read_against_its_first_parent() {
    let repo = Repo::new("merge");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.write("b.txt", b"from topic\n");
    repo.commit("on topic");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("a.txt", b"a\non main\n");
    repo.commit("on main");
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "topic"]);
    let answers = open(
        &mut repo.reader(),
        Scope::Commit {
            revision: "HEAD".into(),
        },
    );
    assert_eq!(answers.status("b.txt"), Some(Status::Added));
    assert_eq!(added(answers.reading("b.txt")), ["from topic\n"]);
    assert_eq!(
        answers.status("a.txt"),
        None,
        "main's own change is the parent's"
    );
}

#[test]
fn a_shallow_clones_oldest_commit_is_refused() {
    let repo = Repo::new("shallow");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    repo.write("a.txt", b"b\n");
    repo.commit("second");
    // As a clone of depth one records it: `HEAD`'s parent is not there to read.
    let head = repo.git(&["rev-parse", "HEAD"]);
    repo.write(".git/shallow", &head);
    let answers = open(
        &mut repo.reader(),
        Scope::Commit {
            revision: "HEAD".into(),
        },
    );
    let [Report::Failed { why, .. }] = &answers.other[..] else {
        panic!("{:?}", answers.other);
    };
    assert!(why.contains("parent is not in this clone"), "{why}");
    assert!(answers.entries.is_empty(), "not read as adding every file");
}

#[test]
fn a_conflicted_file_is_listed_once_and_read_once() {
    let repo = Repo::new("conflict");
    repo.write("a.txt", b"a\n");
    repo.write("b.txt", b"b\n");
    repo.commit("root");
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.write("a.txt", b"topic\n");
    repo.commit("on topic");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("a.txt", b"main\n");
    repo.commit("on main");
    // The merge stops with a conflict, so git says it failed.
    let _ = std::panic::catch_unwind(|| repo.git(&["merge", "-q", "topic"]));
    repo.write("b.txt", b"b\nmore\n");
    let mut reader = repo.reader();
    for scope in [Scope::Unstaged, Scope::Staged] {
        let answers = open(&mut reader, scope.clone());
        let listed: Vec<_> = answers
            .entries
            .iter()
            .filter(|entry| &*entry.path == b"a.txt")
            .collect();
        assert_eq!(listed.len(), 1, "{scope:?}: {listed:?}");
        assert_eq!(listed[0].status, Status::Unmerged);
        assert!(
            matches!(answers.reading("a.txt").content, Content::Conflicted),
            "{scope:?}"
        );
        let again = ask(&mut reader, Request::Refresh { first: Vec::new() });
        assert!(again.readings.is_empty(), "{scope:?}: {:?}", again.paths());
    }
    let unstaged = open(&mut reader, Scope::Unstaged);
    assert_eq!(added(unstaged.reading("b.txt")), ["more\n"]);
}
