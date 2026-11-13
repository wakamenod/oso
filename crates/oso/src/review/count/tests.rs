//! The menu's counts against temporary repositories: each scope, the branch's base, a new
//! opening dropping the last one's, and a revoked root.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the counting threads directly.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use oso_core::buffer::Buffer;
use oso_core::editor::{Count, Editor, ScopeKind};
use oso_core::project::{Found, ProjectId, RootIdentity};

use super::{Counted, Counter, Counting, Job};
use crate::review::tests::Repo;
use crate::review::{Scope, TrustedRoot};

/// A project's id, as the editor gives one for a file found in it.
fn project() -> ProjectId {
    let mut buffer = Buffer::visiting(PathBuf::from("/p/a.rs"));
    buffer.set_baseline(oso_core::buffer::Baseline {
        location: PathBuf::from("/p/a.rs"),
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    let id = editor.tiled().0.buffer();
    editor.request_project(id);
    let request = editor.take_project_requests().pop().unwrap();
    let root = RootIdentity {
        path: PathBuf::from("/p"),
        identity: None,
    };
    let found = Found {
        root,
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
    editor.current().1.project().unwrap()
}

fn counter(repo: &Repo) -> (Counter, mpsc::Receiver<Counted>) {
    let (sent, counted) = mpsc::channel();
    let sent = std::sync::Mutex::new(sent);
    let counter = Counter::new(Some(repo.dir.join("state")), move |c| {
        let _ = sent.lock().unwrap().send(c);
    });
    (counter, counted)
}

fn job(repo: &Repo, generation: u64, counting: Counting) -> Job {
    Job {
        generation,
        project: project(),
        root: TrustedRoot::for_tests(repo.root.clone()),
        counting,
    }
}

#[test]
fn each_scope_counts_the_files_a_review_would_list() {
    let repo = Repo::new("count-scopes");
    repo.write("a", b"1\n");
    repo.write("b", b"1\n");
    repo.commit("first");
    repo.write("a", b"2\n");
    repo.git(&["add", "a"]);
    repo.write("b", b"2\n");
    repo.write("new", b"x\n");
    let (mut counter, counted) = counter(&repo);
    let scope = |kind, scope| Counting::Scope(kind, scope);
    counter.start(vec![
        job(&repo, 1, scope(ScopeKind::Uncommitted, Scope::Uncommitted)),
        job(&repo, 1, scope(ScopeKind::Staged, Scope::Staged)),
        job(&repo, 1, scope(ScopeKind::Unstaged, Scope::Unstaged)),
        job(&repo, 1, Counting::Branch),
    ]);
    let all: Vec<Counted> = (0..4)
        .map(|_| counted.recv_timeout(Duration::from_secs(30)).unwrap())
        .collect();
    let of = |kind| all.iter().find(|c| c.scope == kind).unwrap();
    assert_eq!(of(ScopeKind::Uncommitted).count, Count::Files(3));
    assert_eq!(of(ScopeKind::Staged).count, Count::Files(1));
    assert_eq!(of(ScopeKind::Unstaged).count, Count::Files(1));
    let branch = of(ScopeKind::Branch);
    assert_eq!(branch.count, Count::Files(3));
    assert_eq!(branch.base, Some(Some("main".into())));
    assert!(all.iter().all(|c| c.generation == 1));
}

#[test]
fn a_new_opening_drops_the_counts_of_the_one_before_and_a_revoked_root_fails() {
    let repo = Repo::new("count-epochs");
    repo.write("a", b"1\n");
    repo.commit("first");
    let (mut counter, counted) = counter(&repo);
    let uncommitted = || Counting::Scope(ScopeKind::Uncommitted, Scope::Uncommitted);
    counter.start((0..20).map(|_| job(&repo, 1, uncommitted())).collect());
    let revoked = job(&repo, 2, Counting::Scope(ScopeKind::Staged, Scope::Staged));
    revoked.root.revoke();
    counter.start(vec![revoked]);
    // What the first opening counted before the second began may come, at most one a thread;
    // nothing of it comes after.
    let mut seen = Vec::new();
    while let Ok(c) = counted.recv_timeout(Duration::from_secs(30)) {
        let done = c.generation == 2;
        seen.push(c);
        if done {
            break;
        }
    }
    let last = seen.last().unwrap();
    assert_eq!((last.scope, last.count), (ScopeKind::Staged, Count::Failed));
    assert!(seen.len() <= 1 + super::THREADS, "{seen:?}");
    counter.stop();
    assert!(counted.recv_timeout(Duration::from_millis(500)).is_err());
}
