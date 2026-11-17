//! The tests of keeping reviews that need no thread or window: when a change is saved, and
//! that a saved scope opens as the scope it was.
//!
//! Must not depend on: the store's thread, winit or the renderer.

use std::time::{Duration, Instant};

use super::{AT_MOST, QUIET, Saving, due_at, kind_and_revision};
use crate::git::Id;
use crate::review::Scope;

#[test]
fn a_change_is_saved_once_it_settles_or_at_most_a_while_after_the_first() {
    let start = Instant::now();
    let mut saving = Saving::default();
    assert_eq!(due_at(&saving), None, "nothing changed");
    saving.first = Some(start);
    saving.last = Some(start);
    assert_eq!(due_at(&saving), Some(start + QUIET));
    // Changes go on: the last one moves the time on, up to the most after the first.
    saving.last = Some(start + Duration::from_millis(900));
    assert_eq!(
        due_at(&saving),
        Some(start + Duration::from_millis(900) + QUIET)
    );
    saving.last = Some(start + AT_MOST);
    assert_eq!(due_at(&saving), Some(start + AT_MOST));
}

#[test]
fn a_saved_scope_opens_as_the_scope_it_was() {
    let baseline = Id::parse(&[b'a'; 40]).unwrap();
    for scope in [
        Scope::Session {
            baseline: baseline.clone(),
        },
        Scope::Uncommitted,
        Scope::Staged,
        Scope::Unstaged,
        Scope::Branch {
            base: "main".into(),
        },
        Scope::Commit {
            revision: "HEAD~2".into(),
        },
        Scope::Range {
            from: "a".into(),
            to: "b".into(),
        },
    ] {
        let (kind, revision) = kind_and_revision(&scope);
        assert_eq!(
            super::super::scope_of(kind, revision, Some(baseline.clone())),
            Ok(scope)
        );
    }
}
