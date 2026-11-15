//! Spaces made from remembered sessions: one a repository, a worktree with the repository that
//! records it, inside it or elsewhere, a worktree no remembered repository records on its own,
//! and no space past the ids (ADR 0136).
//!
//! Must not depend on: `app`; these lay out repositories as `git worktree add` leaves them, in
//! a scratch directory of their own, with no git run.

use std::fs;
use std::path::{Path, PathBuf};

use super::from_sessions;
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::record::Remembered;
use crate::sessions::spaces::SpaceId;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oso-migrate-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    /// A main repository at `name`.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.0.join(name);
        fs::create_dir_all(repo.join(".git")).unwrap();
        repo
    }

    /// A worktree of `main` at `path`, recorded back as git records one.
    fn worktree(&self, main: &Path, path: &Path) {
        let name = path.file_name().unwrap();
        let entry = main.join(".git/worktrees").join(name);
        fs::create_dir_all(&entry).unwrap();
        fs::create_dir_all(path).unwrap();
        fs::write(
            entry.join("gitdir"),
            format!("{}\n", path.join(".git").display()),
        )
        .unwrap();
        fs::write(path.join(".git"), format!("gitdir: {}\n", entry.display())).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn session(n: u8, place: &Path) -> Remembered {
    Remembered {
        id: SessionId::from_random([n; 16]),
        name: format!("s{n}"),
        directory: place.to_owned(),
        root: Some(place.to_owned()),
        parent: None,
        origin: Origin::Person,
        pinned: false,
        archived: false,
        changed: 1,
        baseline: None,
        space: None,
    }
}

fn ids(count: u8) -> impl FnMut() -> Option<SpaceId> {
    let mut n = 0;
    move || {
        n += 1;
        (n <= count).then(|| SpaceId::from_random([n; 16]))
    }
}

#[test]
fn each_repository_becomes_a_space_and_its_worktrees_go_with_it() {
    let scratch = Scratch::new("repos");
    let oso = scratch.repo("oso");
    let inside = oso.join(".claude/worktrees/fix");
    scratch.worktree(&oso, &inside);
    let notes = scratch.repo("notes");
    let elsewhere = scratch.0.join("wt/notes-side");
    scratch.worktree(&notes, &elsewhere);
    let mut sessions = vec![
        session(1, &inside),
        session(2, &oso),
        session(3, &notes),
        session(4, &elsewhere),
    ];
    let spaces = from_sessions(&mut sessions, &mut ids(8), 9);
    let names: Vec<&str> = spaces.iter().map(|space| space.name.as_str()).collect();
    assert_eq!(names, ["oso", "notes"]);
    assert_eq!(spaces[0].members[0].root, oso);
    assert!(spaces[0].members[0].identity.is_some());
    assert_eq!(spaces[0].worktrees, [inside]);
    assert_eq!(spaces[1].worktrees, [elsewhere]);
    let of: Vec<&SpaceId> = sessions.iter().map(|s| s.space.as_ref().unwrap()).collect();
    assert_eq!(
        of,
        [&spaces[0].id, &spaces[0].id, &spaces[1].id, &spaces[1].id]
    );
    assert!(
        spaces
            .iter()
            .all(|space| space.directory.is_none() && space.created == 9)
    );
}

#[test]
fn a_worktree_no_repository_records_is_a_space_of_its_own() {
    let scratch = Scratch::new("alone");
    let oso = scratch.repo("oso");
    let stray = scratch.0.join("stray");
    fs::create_dir_all(&stray).unwrap();
    // Its `.git` names oso, which does not record it back.
    fs::write(
        stray.join(".git"),
        format!("gitdir: {}/.git/worktrees/stray\n", oso.display()),
    )
    .unwrap();
    let mut sessions = vec![session(1, &oso), session(2, &stray)];
    let spaces = from_sessions(&mut sessions, &mut ids(8), 1);
    assert_eq!(spaces.len(), 2);
    assert_eq!(spaces[1].members[0].root, stray);
    assert!(spaces[1].worktrees.is_empty());
}

#[test]
fn past_the_ids_a_session_keeps_no_space() {
    let scratch = Scratch::new("ids");
    let a = scratch.repo("a");
    let b = scratch.repo("b");
    let mut sessions = vec![session(1, &a), session(2, &b), session(3, &a)];
    let spaces = from_sessions(&mut sessions, &mut ids(1), 1);
    assert_eq!(spaces.len(), 1);
    let named: Vec<bool> = sessions.iter().map(|s| s.space.is_some()).collect();
    assert_eq!(named, [true, false, true]);
}
