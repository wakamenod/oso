//! Which trust a session started under holds, and what is taken back when it does not, over a
//! real editor's projects and record.
//!
//! Must not depend on: `App`; the sweep runs over the map as the app holds it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oso_core::buffer::{Baseline, Buffer, BufferId};
use oso_core::editor::Editor;
use oso_core::project::{Found, ProjectId, RootIdentity, TrustList};

use super::TrustNow;
use crate::git::TrustedRoot;
use crate::sessions::start::trust::TrustRecord;
use crate::sessions::worker::SessionKey;

/// Two directories, a main repository's and one of its worktrees', removed at the end.
struct Scratch {
    dir: PathBuf,
    main: PathBuf,
    worktree: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("oso-started-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (main, worktree) = (dir.join("main"), dir.join("wt"));
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        Scratch {
            dir,
            main,
            worktree,
        }
    }

    /// The trust of the worktree, as a job under `root`'s makes it.
    fn of_worktree(&self, root: &TrustedRoot) -> TrustedRoot {
        root.for_worktree(self.worktree.clone(), &self.main)
    }

    fn of_main(&self) -> TrustedRoot {
        TrustedRoot::for_tests(self.main.clone())
    }
}

fn at(path: &Path) -> RootIdentity {
    RootIdentity {
        path: path.to_owned(),
        identity: crate::git::identity(&std::fs::symlink_metadata(path).unwrap()),
    }
}

/// A record that holds `roots`, as `trust-project` writes it.
fn record(roots: &[&RootIdentity]) -> TrustList {
    let lines: String = roots
        .iter()
        .map(|root| match root.identity {
            Some((dev, ino)) => format!("{}\tdev={dev} ino={ino}\n", root.path.display()),
            None => format!("{}\n", root.path.display()),
        })
        .collect();
    let (list, skipped) = TrustList::parse(&lines);
    assert_eq!(skipped, 0, "{lines:?}");
    list
}

/// A file of the project `root`, a worktree of `main` if one is given, opened in `editor`.
fn open(
    editor: &mut Editor,
    root: &RootIdentity,
    main: Option<&RootIdentity>,
) -> (BufferId, ProjectId) {
    let file = root.path.join("a.rs");
    let mut buffer = Buffer::visiting(file.clone());
    buffer.set_baseline(Baseline {
        location: file,
        file: None,
    });
    let id = editor.add_buffer(buffer);
    editor.request_project(id);
    let request = editor.take_project_requests().pop().unwrap();
    let found = Found {
        root: root.clone(),
        worktree_of: main.map(|main| main.path.clone()),
        checked_main: main.cloned(),
    };
    editor.project_found(&request, Some(found));
    let project = editor.buffer(id).unwrap().project().unwrap();
    (id, project)
}

fn editor(list: TrustList) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::new("*scratch*"));
    editor.set_trust_list(Some(list));
    editor
}

fn sweep(editor: &Editor, trusts: &mut HashMap<SessionKey, TrustedRoot>) {
    let record = editor
        .trust_list()
        .map_or(TrustRecord::Refused, TrustRecord::Read);
    TrustNow::of(editor, record).sweep(trusts);
}

/// Whether the trust of `root` is kept by the sweep; one that is not is revoked.
fn kept(editor: &Editor, root: &TrustedRoot) -> bool {
    let mut trusts = HashMap::from([(1, root.clone())]);
    sweep(editor, &mut trusts);
    assert_eq!(trusts.is_empty(), root.revoked());
    !trusts.is_empty()
}

#[test]
fn a_worktrees_trust_goes_with_its_main_repositorys() {
    let s = Scratch::new("main");
    let (main, other) = (at(&s.main), at(&s.worktree));
    let root = s.of_worktree(&s.of_main());
    let mut editor = editor(record(&[&main]));
    assert!(kept(&editor, &root), "its main repository is trusted");
    editor.set_trust_list(Some(record(&[&main, &other])));
    assert!(kept(&editor, &root), "another root came");
    editor.set_trust_list(Some(record(&[&main])));
    assert!(kept(&editor, &root), "another root went");
    editor.set_trust_list(Some(TrustList::default()));
    assert!(!kept(&editor, &root));
    editor.set_trust_list(Some(record(&[&main])));
    assert!(!kept(&editor, &root), "one taken back stays so");
}

#[test]
fn a_root_itself_goes_with_its_line() {
    let s = Scratch::new("root");
    let root = s.of_main();
    let mut editor = editor(record(&[&at(&s.main), &at(&s.worktree)]));
    assert!(kept(&editor, &root));
    editor.set_trust_list(Some(record(&[&at(&s.worktree)])));
    assert!(!kept(&editor, &root));
}

#[cfg(unix)]
#[test]
fn a_main_repository_made_again_at_its_path_holds_nothing() {
    let s = Scratch::new("again");
    let root = s.of_worktree(&s.of_main());
    let itself = s.of_main();
    std::fs::rename(&s.main, s.dir.join("moved")).unwrap();
    std::fs::create_dir(&s.main).unwrap();
    let editor = editor(record(&[&at(&s.main)]));
    assert!(!kept(&editor, &root));
    assert!(!kept(&editor, &itself));
}

#[test]
fn a_refused_record_holds_nothing_and_one_not_read_yet_takes_nothing_back() {
    let s = Scratch::new("refused");
    let editor = editor(record(&[&at(&s.main)]));
    let root = s.of_main();
    assert!(TrustNow::of(&editor, TrustRecord::Unread).holds(&root));
    let mut trusts = HashMap::from([(1, root.clone())]);
    TrustNow::of(&editor, TrustRecord::Refused).sweep(&mut trusts);
    assert!(trusts.is_empty() && root.revoked());
}

#[test]
fn an_open_buffer_of_the_root_decides() {
    let s = Scratch::new("open");
    let (main, worktree) = (at(&s.main), at(&s.worktree));
    let mut editor = editor(record(&[&main]));
    // Open, and not checked to be a worktree of the main repository: restricted.
    let (buffer, _) = open(&mut editor, &worktree, None);
    let root = s.of_worktree(&s.of_main());
    assert!(!kept(&editor, &root));
    // Not open any more: the record decides.
    editor.kill_buffer(buffer);
    assert!(kept(&editor, &s.of_worktree(&s.of_main())));
    // Open and trusted through its main repository.
    open(&mut editor, &worktree, Some(&main));
    assert!(kept(&editor, &s.of_worktree(&s.of_main())));
}

#[test]
fn a_restricted_buffer_outweighs_the_project_a_root_was_made_from() {
    let s = Scratch::new("outweighs");
    let (main, worktree) = (at(&s.main), at(&s.worktree));
    let mut editor = editor(record(&[&main]));
    let (buffer, project) = open(&mut editor, &worktree, Some(&main));
    let root = TrustedRoot::of(&editor, project).expect("trusted through its main");
    editor.kill_buffer(buffer);
    // Found again at the same root, its claim not holding now.
    open(&mut editor, &worktree, None);
    assert!(!kept(&editor, &root));
}

#[test]
fn a_worktree_of_an_open_file_holds_by_its_main_repository_once_the_file_is_closed() {
    let s = Scratch::new("closed");
    let (main, worktree) = (at(&s.main), at(&s.worktree));
    let mut editor = editor(record(&[&main]));
    let (buffer, project) = open(&mut editor, &worktree, Some(&main));
    let root = TrustedRoot::of(&editor, project).expect("trusted through its main");
    assert!(kept(&editor, &root), "open");
    editor.kill_buffer(buffer);
    assert!(kept(&editor, &root), "closed");
    editor.set_trust_list(Some(TrustList::default()));
    assert!(!kept(&editor, &root));
}

#[cfg(unix)]
#[test]
fn another_directory_at_a_projects_root_does_not_hold_the_first_ones_trust() {
    let s = Scratch::new("another");
    let first = at(&s.main);
    let mut editor = editor(record(&[&first]));
    let (buffer, project) = open(&mut editor, &first, None);
    let root = TrustedRoot::of(&editor, project).expect("trusted");
    std::fs::rename(&s.main, s.dir.join("moved")).unwrap();
    std::fs::create_dir(&s.main).unwrap();
    // Trusted as it is now, and closed: a project of its own in the table.
    let second = at(&s.main);
    editor.set_trust_list(Some(record(&[&second])));
    let (again, other) = open(&mut editor, &second, None);
    assert_ne!(project, other);
    editor.kill_buffer(buffer);
    editor.kill_buffer(again);
    assert!(!kept(&editor, &root));
}

/// Two directories of one path, one after the other, both open: each root is judged by its own.
#[cfg(unix)]
#[test]
fn of_two_directories_at_a_path_each_roots_own_decides() {
    let s = Scratch::new("own");
    let first = at(&s.main);
    let mut editor = editor(record(&[&first]));
    let (_, project) = open(&mut editor, &first, None);
    let root = TrustedRoot::of(&editor, project).expect("trusted");
    std::fs::rename(&s.main, s.dir.join("moved")).unwrap();
    std::fs::create_dir(&s.main).unwrap();
    let second = at(&s.main);
    editor.set_trust_list(Some(record(&[&second])));
    let (_, again) = open(&mut editor, &second, None);
    let now = TrustedRoot::of(&editor, again).expect("trusted");
    assert!(!kept(&editor, &root), "the first is restricted now");
    assert!(kept(&editor, &now), "the second, whatever the first's is");
}

/// Only another directory at the path is open.
#[cfg(unix)]
#[test]
fn another_directory_open_at_the_path_never_keeps_a_root() {
    let s = Scratch::new("open-other");
    let first = at(&s.main);
    let root = s.of_main();
    let worktrees = s.of_worktree(&root);
    std::fs::rename(&s.main, s.dir.join("moved")).unwrap();
    std::fs::create_dir(&s.main).unwrap();
    let second = at(&s.main);
    // The one there now is restricted: the record holds the first.
    let mut restricted = editor(record(&[&first]));
    open(&mut restricted, &second, None);
    assert!(!kept(&restricted, &root.clone().apart()));
    assert!(
        kept(&restricted, &worktrees.clone().apart()),
        "the worktree's own root is not open, and the record holds its main repository"
    );
    // The one there now is trusted, in the first's place.
    let mut trusted = editor(record(&[&second]));
    open(&mut trusted, &second, None);
    assert!(!kept(&trusted, &root.apart()));
    assert!(!kept(&trusted, &worktrees.apart()));
}

#[test]
fn a_start_given_no_trust_is_not_refused_whatever_the_record_says() {
    let editor = editor(TrustList::default());
    for record in [TrustRecord::Unread, TrustRecord::Refused] {
        let given = TrustNow::of(&editor, record).judge_given(None);
        assert!(given.is_ok_and(|trusted| trusted.is_none()));
    }
}

#[test]
fn a_start_given_a_trust_that_no_longer_holds_is_refused() {
    let s = Scratch::new("given");
    let mut editor = editor(record(&[&at(&s.main)]));
    // Untrusted just before the start.
    let root = s.of_main();
    let given = |editor: &Editor, root: &TrustedRoot| {
        let record = TrustRecord::Read(editor.trust_list().unwrap());
        TrustNow::of(editor, record).judge_given(Some(root.clone()))
    };
    editor.set_trust_list(Some(TrustList::default()));
    assert!(given(&editor, &root).is_err());
    // Taken back before, whatever the record says now.
    editor.set_trust_list(Some(record(&[&at(&s.main)])));
    let taken = s.of_main();
    taken.revoke();
    assert!(given(&editor, &taken).is_err());
    // Untrusted just after: the sweep takes back what the start thread holds.
    let given = given(&editor, &root).unwrap().expect("given");
    let order = given.clone();
    let mut trusts = HashMap::from([(1, given)]);
    editor.set_trust_list(Some(TrustList::default()));
    sweep(&editor, &mut trusts);
    assert!(trusts.is_empty() && order.revoked());
}

/// What a session gives a start shares nothing with what it holds: taking one back leaves the
/// other.
#[test]
fn each_sessions_trust_is_taken_back_by_itself() {
    let s = Scratch::new("apart");
    let (main, worktree) = (at(&s.main), at(&s.worktree));
    let mut editor = editor(record(&[&main]));
    let parent = s.of_main();
    let record = TrustRecord::Read(editor.trust_list().unwrap());
    let child = (TrustNow::of(&editor, record).judge_given(Some(s.of_worktree(&parent))))
        .unwrap()
        .expect("given");
    let mut trusts = HashMap::from([(1, parent.clone()), (2, child.clone())]);
    open(&mut editor, &worktree, None);
    sweep(&editor, &mut trusts);
    assert!(child.revoked() && !parent.revoked());
    assert_eq!(trusts.keys().collect::<Vec<_>>(), [&1]);
}
