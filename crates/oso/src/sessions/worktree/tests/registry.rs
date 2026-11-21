//! Check 4 on a registry not listed in full (ADR 0121, "An incomplete registry read refuses,
//! as amended"): a worktree every other check lets go is kept while Claude Code's registry has
//! more entry files than are read, cannot be listed, has a file that cannot be read, has a live
//! session oso did not start with no directory, or is not known.
//!
//! Must not depend on: `app`, `dispatch` or winit; these read the repository as a job does.

use std::fs;
use std::path::Path;

use super::super::checks::{self, Check, Depth, Held};
use super::super::facts::{
    REGISTRY_CUT, REGISTRY_PLACELESS, REGISTRY_UNKNOWN, REGISTRY_UNLISTABLE, REGISTRY_UNREAD,
    RepoFacts,
};
use super::Scratch;
use crate::sessions::registry::MAX_FILES;

/// The first check of a removal of `path` that refuses, with the registry at `registry`.
fn refusal(s: &Scratch, path: &Path, registry: Option<&Path>) -> Option<Check> {
    let facts = RepoFacts::gather(&s.repo(), None, &[], registry);
    let looked = checks::look(
        &s.repo(),
        None,
        &facts,
        s.listed(path),
        &Held::default(),
        Depth::UntilRefused,
    );
    checks::removable(&looked, &Held::default()).err()
}

#[test]
fn a_registry_not_listed_in_full_keeps_a_merged_worktree() {
    let s = Scratch::new("registry");
    s.with_origin();
    let made = s.create("done").unwrap();
    fs::write(made.path.join("b.txt"), "b\n").unwrap();
    s.git_in(&made.path, &["add", "b.txt"]);
    s.git_in(&made.path, &["commit", "-q", "-m", "work"]);
    s.git(&["merge", "-q", "done"]);
    s.git(&["push", "-q", "origin", "main"]);
    let registry = s.dir.join("registry");
    // Not there: no session wrote to it.
    assert_eq!(refusal(&s, &made.path, Some(&registry)), None);
    // More entry files than are read, none of a process that runs.
    fs::create_dir(&registry).unwrap();
    for n in 0..=MAX_FILES {
        let pid = 4_000_000_000 + n;
        fs::write(
            registry.join(format!("{pid}.json")),
            format!(r#"{{"pid":{pid}}}"#),
        )
        .unwrap();
    }
    let unknown = |why: &str| Some(Check::Unknown(why.to_owned()));
    assert_eq!(
        refusal(&s, &made.path, Some(&registry)),
        unknown(REGISTRY_CUT)
    );
    fs::remove_file(registry.join("4000000000.json")).unwrap();
    assert_eq!(
        refusal(&s, &made.path, Some(&registry)),
        None,
        "as many as are read"
    );
    // There, but not a directory that can be listed.
    let file = s.dir.join("registry-file");
    fs::write(&file, "").unwrap();
    assert_eq!(
        refusal(&s, &made.path, Some(&file)),
        unknown(REGISTRY_UNLISTABLE)
    );
    // Not known at all.
    assert_eq!(refusal(&s, &made.path, None), unknown(REGISTRY_UNKNOWN));
    // A live session oso did not start, with no directory: this test's own process, whose
    // entry says nothing of its start.
    fs::remove_dir_all(&registry).unwrap();
    fs::create_dir(&registry).unwrap();
    let pid = std::process::id();
    let own = registry.join(format!("{pid}.json"));
    fs::write(&own, format!(r#"{{"pid":{pid},"status":"idle"}}"#)).unwrap();
    assert_eq!(
        refusal(&s, &made.path, Some(&registry)),
        unknown(REGISTRY_PLACELESS)
    );
    fs::remove_file(&own).unwrap();
    // An entry file that cannot be read. Root reads it anyway.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let shut = registry.join("4000000000.json");
        fs::write(&shut, r#"{"pid":4000000000}"#).unwrap();
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();
        let want = fs::File::open(&shut)
            .is_err()
            .then(|| Check::Unknown(REGISTRY_UNREAD.into()));
        assert_eq!(refusal(&s, &made.path, Some(&registry)), want);
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
