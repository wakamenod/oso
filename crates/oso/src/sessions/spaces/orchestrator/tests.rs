//! The orchestrator's header and instructions, the checks its directory passes just before it
//! starts, and which repositories it is given (ADR 0136, step 3).
//!
//! Must not depend on: `app`; these use a scratch directory of their own.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{
    MAX_INSTRUCTIONS, Repository, TEXT, check_before_start, header, instructions_section,
    read_instructions, still_there,
};
use crate::sessions::spaces::dir::{self, Refusal};
use crate::sessions::spaces::{Member, SpaceId, Stored};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oso-orchestrator-{name}-{}-{:?}",
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
fn the_header_names_the_space_its_title_and_its_repositories_with_their_trust() {
    let mut space = Stored::of_repository(
        SpaceId::from_random([1; 16]),
        "fix-login".into(),
        Member::at(Path::new("/b")),
        1,
    );
    space.title = "Login\u{202E} error".into();
    let repositories = [
        Repository {
            name: "back",
            root: Path::new("/src/back"),
            trusted: true,
        },
        Repository {
            name: "front",
            root: Path::new("/src/front"),
            trusted: false,
        },
    ];
    let text = header(&space, &repositories);
    assert!(text.contains("# This space: fix-login"));
    assert!(
        text.contains("in the person's words: Login error"),
        "{text}"
    );
    assert!(text.contains("- back: /src/back (added to your working directories)"));
    assert!(text.contains("- front: /src/front (not trusted in oso yet"));
    assert!(TEXT.contains("handoff.md"));
}

#[test]
fn instructions_are_cut_said_to_be_editable_and_a_linked_file_is_refused() {
    assert_eq!(instructions_section("  \n"), "");
    let long = "x".repeat(MAX_INSTRUCTIONS + 10);
    let section = instructions_section(&long);
    assert!(section.contains("agents in oso can edit that file too"));
    assert!(section.len() < MAX_INSTRUCTIONS + 400);
    let scratch = Scratch::new("instructions");
    let file = scratch.0.join("a.md");
    assert_eq!(read_instructions(&file), Ok(String::new()), "missing: none");
    fs::write(&file, "Fix the login error\n").unwrap();
    assert_eq!(read_instructions(&file).unwrap(), "Fix the login error\n");
    let link = scratch.0.join("b.md");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(read_instructions(&link).is_err());
}

#[test]
fn a_shared_directory_anywhere_above_refuses_the_start_sticky_or_not() {
    let scratch = Scratch::new("shared");
    let root = scratch.0.join("spaces");
    let (path, identity) = dir::make(&root, "x", &[]).unwrap();
    check_before_start(&path, identity, &[]).unwrap();
    // A sticky, world-writable directory on the way, as /tmp is.
    fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o1777)).unwrap();
    let refused = check_before_start(&path, identity, &[]).unwrap_err();
    assert!(matches!(refused, Refusal::Shared(_)), "{refused}");
    fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_repository_put_in_anothers_place_is_left_out() {
    let scratch = Scratch::new("members");
    let kept = scratch.0.join("kept");
    let moved = scratch.0.join("moved");
    fs::create_dir_all(&kept).unwrap();
    fs::create_dir_all(&moved).unwrap();
    let identity = |path: &Path| dir::identity(&fs::symlink_metadata(path).unwrap());
    let members = vec![
        Member {
            root: kept.clone(),
            identity: identity(&kept),
        },
        Member {
            root: moved.clone(),
            identity: identity(&moved),
        },
        Member::at(&scratch.0.join("new")),
    ];
    // Another directory at `moved`'s place; `new` is not there at all.
    fs::rename(&moved, scratch.0.join("away")).unwrap();
    fs::create_dir(&moved).unwrap();
    let (given, gone) = still_there(&members);
    assert_eq!(
        given.iter().map(|m| m.root.clone()).collect::<Vec<_>>(),
        [kept]
    );
    assert_eq!(gone, [moved, scratch.0.join("new")]);
}
