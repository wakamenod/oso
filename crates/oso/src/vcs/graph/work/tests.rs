//! A graph's thread over a temporary repository: a read gives every commit's row, a page asked
//! past the end of history is answered at once with nothing new, and a root no longer trusted
//! runs no git.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use std::sync::mpsc;
use std::time::Duration;

use super::{GraphReport, GraphRequest, GraphWorker};
use crate::git::TrustedRoot;
use crate::review::tests::Repo;

fn repo(name: &str) -> Repo {
    let repo = Repo::new(name);
    repo.write("a.txt", b"one\n");
    repo.commit("first");
    repo.write("a.txt", b"two\n");
    repo.commit("second");
    repo
}

fn worker(root: TrustedRoot, repo: &Repo) -> (GraphWorker, mpsc::Receiver<GraphReport>) {
    let (sent, got) = mpsc::channel();
    let worker = GraphWorker::start(root, Some(repo.dir.join("state")), move |report| {
        sent.send(report).is_ok()
    })
    .unwrap();
    (worker, got)
}

fn read(generation: u64) -> GraphRequest {
    GraphRequest::Read {
        generation,
        refs: vec!["HEAD".into()],
        title: "Commits of HEAD".into(),
    }
}

fn next(got: &mpsc::Receiver<GraphReport>) -> GraphReport {
    got.recv_timeout(Duration::from_secs(30)).expect("a report")
}

#[test]
fn a_read_gives_each_commit_and_a_page_past_the_end_nothing_new() {
    let repo = repo("graph-read");
    let (worker, got) = worker(TrustedRoot::for_tests(repo.root.clone()), &repo);
    assert!(worker.ask(read(1)));
    let GraphReport::Laid { generation, laid } = next(&got) else {
        panic!("not laid");
    };
    assert_eq!(generation, 1);
    let shown = laid.text.text().to_string();
    let rows: Vec<&str> = shown.lines().collect();
    assert_eq!(rows[0], "Commits of HEAD");
    assert!(
        rows[1].starts_with("● ") && rows[1].contains(" HEAD main second "),
        "{shown}"
    );
    assert!(
        rows[2].starts_with("● ") && rows[2].contains(" first "),
        "{shown}"
    );
    assert!(!laid.built.more);
    // Nothing is left to read: the page is answered at once, with nothing new.
    assert!(worker.ask(GraphRequest::More { generation: 1 }));
    assert!(matches!(
        next(&got),
        GraphReport::Unchanged { generation: 1 }
    ));
}

#[test]
fn a_root_no_longer_trusted_runs_no_git() {
    let repo = repo("graph-revoked");
    let root = TrustedRoot::for_tests(repo.root.clone());
    let (worker, got) = worker(root.clone(), &repo);
    root.revoke();
    assert!(worker.ask(read(2)));
    assert!(matches!(
        next(&got),
        GraphReport::Failed { generation: 2, .. }
    ));
}
