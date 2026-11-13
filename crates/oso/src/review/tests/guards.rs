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
    // Unix only from here, as the trust tests elsewhere are: without a drive, the path is not
    // absolute on Windows, and a record of it trusts nothing there.
    if cfg!(not(unix)) {
        return;
    }
    let (editor, project) = editor_in_a_project("/home/me/src/oso\tdev=1 ino=7\n");
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

#[cfg(unix)]
#[test]
fn a_file_with_another_name_is_withheld_until_it_has_one() {
    use std::fs;
    let repo = Repo::new("hard-link");
    repo.write("keep.txt", b"k\n");
    repo.commit("root");
    repo.write(".env", b"SECRET=1\n");
    fs::hard_link(repo.root.join(".env"), repo.root.join("notes.txt")).unwrap();
    let mut reader = repo.reader();
    let answers = open(&mut reader, Scope::Uncommitted);
    assert!(matches!(
        answers.reading("notes.txt").content,
        Content::Withheld
    ));
    let again = ask(&mut reader, Request::Refresh { first: Vec::new() });
    assert!(again.readings.is_empty(), "{:?}", again.paths());
    fs::remove_file(repo.root.join(".env")).unwrap();
    let alone = ask(&mut reader, Request::Refresh { first: Vec::new() });
    assert_eq!(added(alone.reading("notes.txt")), ["SECRET=1\n"]);
}

/// A script at `path` that notes each run in `mark` and passes its input through, as a clean
/// filter must.
#[cfg(unix)]
fn marking_script(path: &std::path::Path, mark: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let text = format!("#!/bin/sh\necho ran >> '{}'\ncat\n", mark.display());
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn no_hook_monitor_or_embedded_repository_program_runs() {
    use std::fs;
    use std::time::{Duration, SystemTime};
    let repo = Repo::new("no-programs");
    let (script, mark) = (repo.dir.join("mark.sh"), repo.dir.join("ran"));
    marking_script(&script, &mark);
    let script = script.to_str().unwrap();
    repo.write("a.txt", b"a\n");
    let vendor = Repo {
        dir: repo.dir.join("unused"),
        root: repo.root.join("vendor"),
    };
    fs::create_dir(&vendor.root).unwrap();
    vendor.git(&["init", "-q", "-b", "main"]);
    vendor.write("v.txt", b"v\n");
    vendor.commit("vendored");
    repo.write(
        ".gitmodules",
        b"[submodule \"vendor\"]\n\tpath = vendor\n\turl = ./vendor\n\tignore = none\n",
    );
    repo.commit("root");
    // `git add` runs this hook when it writes an index.
    for repo in [&repo, &vendor] {
        repo.git(&["config", "core.fsmonitor", script]);
        fs::copy(script, repo.root.join(".git/hooks/post-index-change")).unwrap();
    }
    vendor.git(&["config", "filter.mark.clean", script]);
    vendor.write(".git/info/attributes", b"* filter=mark\n");
    // Changed, but not in size: only hashing it through its filter tells.
    vendor.write("v.txt", b"w\n");
    let old = SystemTime::now() - Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(vendor.root.join("v.txt"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    repo.write("a.txt", b"b\n");
    repo.write("new.txt", b"n\n");
    let _ = fs::remove_file(&mark);
    let mut reader = repo.reader();
    for scope in [Scope::Uncommitted, Scope::Unstaged] {
        let answers = open(&mut reader, scope.clone());
        assert_eq!(answers.status("a.txt"), Some(Status::Modified), "{scope:?}");
    }
    let branch = open(
        &mut reader,
        Scope::Branch {
            base: "main".into(),
        },
    );
    assert_eq!(branch.status("new.txt"), Some(Status::Added));
    let taken = ask(&mut reader, Request::Baseline);
    assert!(
        matches!(taken.other[..], [Report::Baseline { .. }]),
        "{:?}",
        taken.other
    );
    assert!(
        !mark.exists(),
        "{}",
        fs::read_to_string(&mark).unwrap_or_default()
    );
}

#[test]
fn a_revoked_root_runs_nothing_more() {
    use super::super::Reviewer;
    use std::sync::mpsc;
    use std::time::Duration;
    let repo = Repo::new("revoked");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    let root = TrustedRoot::for_tests(repo.root.clone());
    let (sent, got) = mpsc::channel();
    let reviewer = Reviewer::start(root.clone(), Some(repo.dir.join("state")), move |report| {
        sent.send(format!("{report:?}")).is_ok()
    })
    .unwrap();
    root.revoke();
    assert!(reviewer.ask(Request::Open(Scope::Uncommitted)));
    let answer = got.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(answer.contains("no longer trusted"), "{answer}");
}

#[cfg(unix)]
#[test]
fn a_root_whose_parent_git_cannot_stop_at_is_refused() {
    let dir = super::scratch_dir("ceiling");
    let root = dir.join("a:b").join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let mut reader = Reader::new(&root, Some(&dir.join("state")));
    let answers = open(&mut reader, Scope::Uncommitted);
    let [Report::Failed { why, .. }] = &answers.other[..] else {
        panic!("{:?}", answers.other);
    };
    assert!(why.contains("parent's path"), "{why}");
    std::fs::remove_dir_all(dir).unwrap();
}
