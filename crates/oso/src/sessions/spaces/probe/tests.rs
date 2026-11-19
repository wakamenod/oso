//! What the space commands look up on the worker: a directory made under the next free name,
//! a repository's main checkout told from a worktree and a plain directory, and the
//! instructions' private directory (ADR 0136, step 1b).
//!
//! Must not depend on: `app`; these use a scratch directory of their own.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{instructions_file, main_checkout, main_checkout_as_named, make_directory};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oso-space-probe-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self(dir.canonicalize().unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_taken_directory_name_gives_the_next_free_one() {
    let scratch = Scratch::new("names");
    let root = scratch.0.join("spaces");
    let first = make_directory(&root, "fix", &[], &|_| false, false).unwrap();
    assert_eq!(
        (first.name.as_str(), first.path.clone()),
        ("fix", root.join("fix"))
    );
    let second = make_directory(&root, "fix", &[], &|_| false, false).unwrap();
    assert_eq!(second.name, "fix-2");
    // A name another space has is passed over too, though nothing is at it.
    let third = make_directory(&root, "fix", &[], &|name| name == "fix-3", false).unwrap();
    assert_eq!(third.name, "fix-4");
    assert!(third.identity.is_some());
}

#[test]
fn a_shared_root_is_said_and_nothing_is_made() {
    let scratch = Scratch::new("shared");
    let root = scratch.0.join("spaces");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o770)).unwrap();
    let why = make_directory(&root, "x", &[], &|_| false, false).unwrap_err();
    assert!(why.contains("[spaces] root"), "{why}");
    assert!(!root.join("x").exists());
}

#[test]
fn only_a_main_checkout_is_a_repository() {
    let scratch = Scratch::new("repos");
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let member = main_checkout(&repo).unwrap();
    assert_eq!(member.root, repo);
    assert!(member.identity.is_some());
    let worktree = scratch.0.join("wt");
    fs::create_dir(&worktree).unwrap();
    fs::write(worktree.join(".git"), "gitdir: /x/.git/worktrees/wt\n").unwrap();
    assert!(main_checkout(&worktree).unwrap_err().contains("worktree"));
    let plain = scratch.0.join("plain");
    fs::create_dir(&plain).unwrap();
    assert!(main_checkout(&plain).unwrap_err().contains("no repository"));
    assert!(main_checkout(&scratch.0.join("missing")).is_err());
}

#[test]
fn instructions_are_kept_in_a_private_directory_of_the_state_directory() {
    let scratch = Scratch::new("instructions");
    let state = scratch.0.join("oso");
    let file = instructions_file(&state, "abc").unwrap();
    assert_eq!(file, state.join("space-instructions/abc.md"));
    let mode = fs::metadata(file.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn the_root_and_a_home_holding_directory_are_no_repository() {
    assert!(
        main_checkout(std::path::Path::new("/"))
            .unwrap_err()
            .contains("home")
    );
    if let Some(home) = oso_platform::privacy::home().filter(|home| home.join(".git").is_dir()) {
        assert!(main_checkout(&home).is_err());
    }
}

#[test]
fn an_instructions_file_that_is_a_link_is_refused() {
    let scratch = Scratch::new("link");
    let state = scratch.0.join("oso");
    let file = instructions_file(&state, "abc").unwrap();
    let elsewhere = scratch.0.join("elsewhere.md");
    fs::write(&elsewhere, "x").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
    assert!(instructions_file(&state, "abc").is_err());
}

#[test]
fn an_agent_names_a_repository_by_its_real_path_only() {
    let scratch = Scratch::new("named");
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let member = main_checkout_as_named(&repo).unwrap();
    assert_eq!(member.root, repo);
    let link = scratch.0.join("link");
    std::os::unix::fs::symlink(&repo, &link).unwrap();
    let why = main_checkout_as_named(&link).unwrap_err();
    assert!(why.contains("goes through the link"), "{why}");
    assert!(
        main_checkout(&link).is_ok(),
        "the person's prompt follows it"
    );
    // A link above the repository is one on the way too.
    let above = scratch.0.join("above");
    std::os::unix::fs::symlink(&scratch.0, &above).unwrap();
    assert!(main_checkout_as_named(&above.join("repo")).is_err());
    let dotted = scratch.0.join("x").join("..").join("repo");
    fs::create_dir(scratch.0.join("x")).unwrap();
    assert!(main_checkout_as_named(&dotted).unwrap_err().contains(".."));
    assert!(main_checkout_as_named(&scratch.0.join("gone")).is_err());
    let deep: PathBuf = std::iter::repeat_n("a", super::MAX_NAMED_DEPTH).collect();
    let why = main_checkout_as_named(&scratch.0.join(deep)).unwrap_err();
    assert!(why.contains("directories deep"), "{why}");
    let why = main_checkout_as_named(Path::new("/net/host/repo")).unwrap_err();
    assert!(why.contains("local repositories only"), "{why}");
    let odd = scratch.0.join("we\u{200b}b");
    fs::create_dir_all(odd.join(".git")).unwrap();
    assert!(
        main_checkout_as_named(&odd)
            .unwrap_err()
            .contains("does not show")
    );
}
