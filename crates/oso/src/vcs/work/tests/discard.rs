//! Discarding over temporary repositories (ADR 0127, step 1c): a file kept first and written
//! back with what it held kept too, hunks, staged changes and untracked files, a file that may
//! hold secrets with no copy, files that get no copy and stop it, and paths never followed.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the thread's state directly.

use std::path::Path;

use super::super::{Report, Request, State};
use super::{answer, cached, open_a, read, run, state, text, two_hunks};
use crate::git::TrustedRoot;
use crate::review::tests::Repo;
use crate::vcs::layout::{Laid, Section, View};
use crate::vcs::stage::Target;

/// Discards `targets` of `laid`, as the UI asks once the person said `y`.
fn discard(
    state: &mut State,
    root: &Path,
    laid: &Laid,
    targets: &[Target],
    view: &View,
) -> (Laid, Vec<Report>) {
    answer(
        state,
        root,
        Request::Discard {
            generation: 3,
            built: std::sync::Arc::clone(&laid.built),
            targets: targets.to_vec(),
            view: view.clone(),
        },
    )
}

/// The repository's files as written, line breaks and all: a machine's `core.autocrlf`, as
/// Windows runners set, would make git write CRLF where the tests compare bytes.
fn lf_only(repo: &Repo) {
    repo.git(&["config", "core.autocrlf", "false"]);
}

fn kept(repo: &Repo) -> Vec<crate::vcs::discarded::Entry> {
    crate::vcs::discarded::read(&repo.dir.join("state")).unwrap()
}

#[test]
fn a_discarded_file_is_kept_first_and_written_back_with_what_it_held_kept_too() {
    let repo = Repo::new("vcs-discard-file");
    lf_only(&repo);
    repo.write("a.txt", b"one\n");
    repo.commit("first");
    repo.write("a.txt", b"edited\n");
    let mut state = state(&repo);
    let view = View::default();
    let laid = read(&mut state, &repo.root, &view);
    let target = Target::File {
        section: Section::Unstaged,
        file: 0,
    };
    let (laid, reports) = discard(&mut state, &repo.root, &laid, &[target], &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { said }] if said.contains("git-discarded")),
        "{reports:?}"
    );
    assert_eq!(std::fs::read(repo.root.join("a.txt")).unwrap(), b"one\n");
    assert!(!text(&laid).contains("Unstaged changes"));
    let entries = kept(&repo);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, b"a.txt");
    let blob = repo.git(&["cat-file", "blob", &entries[0].id]);
    assert_eq!(blob, b"edited\n");
    // Written back over what the file holds now, which is kept too.
    let git = state.git.clone().unwrap();
    let root = TrustedRoot::for_tests(repo.root.clone());
    let stop = std::sync::atomic::AtomicBool::new(false);
    let state_dir = repo.dir.join("state");
    let said =
        crate::vcs::keep::restore(&git, &root, (&entries[0], false), Some(&state_dir), &stop)
            .unwrap();
    assert_eq!(said, "Wrote a.txt back");
    assert_eq!(std::fs::read(repo.root.join("a.txt")).unwrap(), b"edited\n");
    let entries = kept(&repo);
    assert_eq!(entries.len(), 2);
    assert_eq!(repo.git(&["cat-file", "blob", &entries[1].id]), b"one\n");
}

#[test]
fn hunks_lines_staged_changes_and_untracked_files_are_discarded() {
    let repo = two_hunks("vcs-discard-kinds");
    lf_only(&repo);
    repo.write("new.txt", b"new\n");
    repo.write("s.txt", b"s\n");
    repo.git(&["add", "s.txt"]);
    let mut state = state(&repo);
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    // The first hunk alone of a.txt: `two` goes back to `2`, `fourteen` stays.
    let hunk = Target::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk: 0,
    };
    let (laid, reports) = discard(&mut state, &repo.root, &laid, &[hunk], &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    let work = std::fs::read_to_string(repo.root.join("a.txt")).unwrap();
    assert!(
        work.contains("\n2\n") && work.contains("fourteen"),
        "{work}"
    );
    // The untracked file and the staged new one go.
    let untracked = Target::Section(Section::Untracked);
    let staged = Target::Section(Section::Staged);
    let (_, reports) = discard(&mut state, &repo.root, &laid, &[untracked, staged], &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    assert!(!repo.root.join("new.txt").exists());
    assert!(!repo.root.join("s.txt").exists());
    assert_eq!(cached(&repo), "");
    let paths: Vec<Vec<u8>> = kept(&repo).into_iter().map(|e| e.path).collect();
    assert!(paths.contains(&b"new.txt".to_vec()) && paths.contains(&b"s.txt".to_vec()));
}

#[test]
fn a_file_that_may_hold_secrets_is_discarded_with_no_copy() {
    let repo = Repo::new("vcs-discard-secret");
    lf_only(&repo);
    repo.write(".env", b"A=1\n");
    repo.commit("first");
    repo.write(".env", b"A=2\n");
    let mut state = state(&repo);
    let view = View::default();
    let laid = read(&mut state, &repo.root, &view);
    let target = Target::File {
        section: Section::Unstaged,
        file: 0,
    };
    let (_, reports) = discard(&mut state, &repo.root, &laid, &[target], &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { said }] if said == "Discarded a file"),
        "{reports:?}"
    );
    assert_eq!(std::fs::read(repo.root.join(".env")).unwrap(), b"A=1\n");
    assert!(kept(&repo).is_empty());
}

#[cfg(unix)]
#[test]
fn a_path_through_a_link_is_never_deleted() {
    let repo = Repo::new("vcs-discard-link");
    lf_only(&repo);
    repo.write("kept.txt", b"k\n");
    repo.commit("first");
    let outside = repo.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("victim.txt"), b"v\n").unwrap();
    std::os::unix::fs::symlink(&outside, repo.root.join("link")).unwrap();
    let top = repo.root.canonicalize().unwrap();
    assert!(crate::vcs::keep::inside(&top, b"link/victim.txt").is_err());
    assert!(crate::vcs::keep::inside(&top, b"../x").is_err());
    assert!(crate::vcs::keep::inside(&top, b"a/b.txt").is_ok());
    assert!(outside.join("victim.txt").exists());
}

#[test]
fn a_staged_hunk_goes_from_the_index_and_the_work_tree_whatever_else_is_unstaged() {
    let repo = two_hunks("vcs-discard-staged-hunk");
    lf_only(&repo);
    let mut state = state(&repo);
    // The first hunk staged; the second stays unstaged, so index and work tree differ.
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    let first = Target::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk: 0,
    };
    run(&mut state, &repo.root, &laid, &[first], true, &view);
    let view = open_a(Section::Staged);
    let laid = read(&mut state, &repo.root, &view);
    let staged = Target::Hunk {
        section: Section::Staged,
        file: 0,
        hunk: 0,
    };
    let (_, reports) = discard(&mut state, &repo.root, &laid, &[staged], &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    assert_eq!(cached(&repo), "");
    let work = std::fs::read_to_string(repo.root.join("a.txt")).unwrap();
    assert!(
        work.contains("\n2\n") && work.contains("fourteen"),
        "{work}"
    );
}

#[cfg(unix)]
#[test]
fn a_file_with_two_names_or_for_its_owner_alone_gets_no_copy_and_nothing_runs() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new("vcs-discard-links");
    lf_only(&repo);
    repo.write("kept.txt", b"k\n");
    repo.commit("first");
    std::fs::write(repo.dir.join("elsewhere"), b"theirs\n").unwrap();
    std::fs::hard_link(repo.dir.join("elsewhere"), repo.root.join("two.txt")).unwrap();
    let mut state = state(&repo);
    let view = View::default();
    let laid = read(&mut state, &repo.root, &view);
    let untracked = Target::Section(Section::Untracked);
    let (_, reports) = discard(&mut state, &repo.root, &laid, &[untracked], &view);
    assert!(
        matches!(reports.as_slice(), [Report::Failed { why }] if why.contains("more than one name")),
        "{reports:?}"
    );
    assert!(repo.root.join("two.txt").exists());
    std::fs::remove_file(repo.root.join("two.txt")).unwrap();
    repo.write("own.txt", b"mine\n");
    let own = repo.root.join("own.txt");
    std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o600)).unwrap();
    let laid = read(&mut state, &repo.root, &view);
    let (_, reports) = discard(&mut state, &repo.root, &laid, &[untracked], &view);
    assert!(
        matches!(reports.as_slice(), [Report::Failed { why }] if why.contains("owner's eyes only")),
        "{reports:?}"
    );
    assert!(own.exists());
    assert!(kept(&repo).is_empty());
}

#[test]
fn nothing_of_gits_own_directory_is_followed() {
    let top = std::env::temp_dir();
    for path in [
        &b".git/hooks/pre-commit"[..],
        b"a/.GIT/config",
        b"x/git~1/y",
        b".git./z",
    ] {
        assert!(crate::vcs::keep::inside(&top, path).is_err(), "{path:?}");
    }
}
