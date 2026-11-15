//! A space's directory: made private under its root, checked before each use, and refused
//! when the root is shared, when it is missing or replaced, when it is a repository or inside
//! one of the space's (ADR 0136, Q1).
//!
//! Must not depend on: `app`; these use a scratch directory of their own.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{Refusal, check, make};
use crate::sessions::spaces::Member;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oso-space-dir-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_space_directory_is_made_private_and_checks_until_replaced() {
    let scratch = Scratch::new("made");
    let root = scratch.0.join("spaces");
    let (path, identity) = make(&root, "fix-login", &[]).unwrap();
    assert_eq!(path, root.join("fix-login"));
    assert_eq!((mode(&root), mode(&path)), (0o700, 0o700));
    assert!(identity.is_some());
    check(&path, identity, &[]).unwrap();
    // Moved away: missing. Another put in its place: replaced.
    let away = scratch.0.join("away");
    fs::rename(&path, &away).unwrap();
    assert!(matches!(
        check(&path, identity, &[]),
        Err(Refusal::Missing(_))
    ));
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let refused = check(&path, identity, &[]).unwrap_err();
    assert!(matches!(refused, Refusal::Replaced(_)), "{refused}");
}

#[test]
fn a_root_others_can_write_is_refused_and_nothing_is_made() {
    let scratch = Scratch::new("shared");
    let root = scratch.0.join("spaces");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o770)).unwrap();
    let refused = make(&root, "x", &[]).unwrap_err();
    assert!(matches!(refused, Refusal::Shared(_)), "{refused}");
    assert!(refused.to_string().contains("[spaces] root"));
    assert!(!root.join("x").exists());
    // A space made before the root was opened up refuses to start there too.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let (path, identity) = make(&root, "x", &[]).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        check(&path, identity, &[]),
        Err(Refusal::Shared(_))
    ));
}

#[test]
fn a_loose_space_directory_is_refused_not_repaired() {
    let scratch = Scratch::new("loose");
    let root = scratch.0.join("spaces");
    let (path, identity) = make(&root, "x", &[]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        check(&path, identity, &[]),
        Err(Refusal::Refused(..))
    ));
    assert_eq!(mode(&path), 0o755, "left as it was");
}

#[test]
fn a_repository_or_a_place_inside_one_of_the_spaces_is_refused() {
    let scratch = Scratch::new("repo");
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let refused = make(&repo, "x", &[]).unwrap_err();
    assert!(matches!(refused, Refusal::Repository(_)), "{refused}");
    let root = scratch.0.join("spaces");
    let member = Member::at(&scratch.0);
    let refused = make(&root, "x", &[member]).unwrap_err();
    assert!(matches!(refused, Refusal::InsideRepository(_)), "{refused}");
    let (path, identity) = make(&root, "y", &[]).unwrap();
    fs::create_dir(path.join(".git")).unwrap();
    assert!(matches!(
        check(&path, identity, &[]),
        Err(Refusal::Repository(_))
    ));
}
