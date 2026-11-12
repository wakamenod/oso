//! Reviews that must refuse or hold back: an untrusted or replaced root, a root inside another
//! repository, secrets by a rename's old name, attributes that would hide a change, a stopped
//! request, and what a review holds.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the review's reader and thread
//! directly.

use std::path::PathBuf;

use super::super::work::Reader;
use super::super::{Content, Report, Request, Scope, TrustedRoot};
use super::{Repo, Status, added, ask, open};

/// An editor with one buffer in a project at `/home/me/src/oso`, whose trust is as `record`
/// reads.
fn editor_in_a_project(record: &str) -> (oso_core::editor::Editor, oso_core::project::ProjectId) {
    use oso_core::buffer::{Baseline, Buffer};
    use oso_core::editor::Editor;
    use oso_core::project::{Found, RootIdentity, TrustList};
    let mut buffer = Buffer::visiting(PathBuf::from("/home/me/src/oso/a.rs"));
    buffer.set_baseline(Baseline {
        location: PathBuf::from("/home/me/src/oso/a.rs"),
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    editor.set_home(Some(PathBuf::from("/home/me")));
    let id = editor.tiled().0.buffer();
    editor.request_project(id);
    let request = editor.take_project_requests().pop().unwrap();
    let found = Found {
        root: RootIdentity {
            path: PathBuf::from("/home/me/src/oso"),
            identity: cfg!(unix).then_some((1, 7)),
        },
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
    editor.set_trust_list(Some(TrustList::parse(record).0));
    let project = editor.tiled().1.project().unwrap();
    (editor, project)
}

#[test]
fn only_a_trusted_project_gives_a_root_to_review() {
    let (editor, project) = editor_in_a_project("");
    assert!(TrustedRoot::of(&editor, project).is_none(), "restricted");
    let record = if cfg!(unix) {
        "/home/me/src/oso\tdev=1 ino=7\n"
    } else {
        "/home/me/src/oso\n"
    };
    let (editor, project) = editor_in_a_project(record);
    let root = TrustedRoot::of(&editor, project).expect("trusted");
    assert_eq!(root.path(), PathBuf::from("/home/me/src/oso"));
}

#[test]
fn a_renamed_secret_is_withheld_by_its_old_name() {
    let repo = Repo::new("renamed-secret");
    repo.write(".env", b"TOKEN=1\nOTHER=2\nMORE=3\n");
    repo.commit("root");
    repo.git(&["mv", ".env", "notes.txt"]);
    repo.write("notes.txt", b"TOKEN=1\nOTHER=2\nMORE=3\nmore\n");
    let answers = open(&mut repo.reader(), Scope::Staged);
    assert_eq!(answers.status("notes.txt"), Some(Status::Renamed));
    assert!(matches!(
        answers.reading("notes.txt").content,
        Content::Withheld
    ));
}

#[test]
fn attributes_do_not_hide_a_change_as_binary() {
    let repo = Repo::new("attributes");
    repo.write("a.rs", b"fn a() {}\n");
    repo.commit("root");
    repo.write(".git/info/attributes", b"* -diff binary\n");
    repo.write("a.rs", b"fn a() {}\nfn b() {}\n");
    let answers = open(&mut repo.reader(), Scope::Uncommitted);
    assert_eq!(added(answers.reading("a.rs")), ["fn b() {}\n"]);
}

#[test]
fn a_root_inside_another_repository_is_refused() {
    let repo = Repo::new("nested-root");
    repo.write("inner/a.txt", b"a\n");
    repo.commit("root");
    repo.write("inner/a.txt", b"b\n");
    let mut reader = Reader::new(&repo.root.join("inner"), Some(&repo.dir.join("state")));
    let answers = open(&mut reader, Scope::Uncommitted);
    assert!(matches!(answers.other[..], [Report::Failed { .. }]));
    assert!(answers.entries.is_empty());
}

#[test]
fn a_request_told_to_stop_reads_no_file() {
    let repo = Repo::new("stop");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    repo.write("a.txt", b"b\n");
    let mut reader = repo.reader();
    let mut stopped = None;
    let mut files = 0;
    reader.answer(Request::Open(Scope::Uncommitted), &|| true, &mut |report| {
        match report {
            Report::File { .. } => files += 1,
            Report::Read { stopped: s, .. } => stopped = Some(s),
            _ => {}
        }
        true
    });
    assert_eq!((files, stopped), (0, Some(true)));
    // The next refresh reads what the stopped one left.
    let again = ask(&mut reader, Request::Refresh { first: Vec::new() });
    assert_eq!(again.paths(), ["a.txt"]);
    assert_eq!(again.stopped, Some(false));
}

#[test]
fn what_a_review_holds_is_counted_and_let_go() {
    let repo = Repo::new("held");
    repo.write("a.txt", b"one\n");
    repo.commit("root");
    repo.write("a.txt", b"two\n");
    let mut reader = repo.reader();
    open(&mut reader, Scope::Uncommitted);
    // Both sides, and each hunk line with its cost.
    assert_eq!(reader.held(), 4 + 4 + 2 * (4 + 16));
    repo.commit("second");
    ask(&mut reader, Request::Refresh { first: Vec::new() });
    assert_eq!(reader.held(), 0, "a file no longer listed is let go");
}

#[cfg(unix)]
#[test]
fn a_root_put_in_the_trusted_ones_place_runs_no_git() {
    use super::super::Reviewer;
    use std::fs;
    use std::sync::mpsc;
    use std::time::Duration;
    let repo = Repo::new("replaced-root");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    let root = TrustedRoot::for_tests(repo.root.clone());
    fs::rename(&repo.root, repo.dir.join("moved")).unwrap();
    fs::create_dir(&repo.root).unwrap();
    let (sent, got) = mpsc::channel();
    let reviewer = Reviewer::start(root, None, move |report| {
        sent.send(format!("{report:?}")).is_ok()
    })
    .unwrap();
    assert!(reviewer.ask(Request::Open(Scope::Uncommitted)));
    let answer = got.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(
        answer.contains("no longer the directory that was trusted"),
        "{answer}"
    );
}
