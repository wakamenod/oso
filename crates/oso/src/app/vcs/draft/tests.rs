//! What lets a draft from `draft_set` into a commit's message: the session asked in its own
//! worktree, a trusted project, no commit running, and a request at most 15 minutes old.
//!
//! Must not depend on: winit or the window; these are the checks' own tests.

use std::time::Duration;

use super::{ASKED_FOR, Found};
use crate::commands::vcs::RESTRICTED;

fn found() -> Found {
    Found {
        same_worktree: true,
        trusted: true,
        runs: false,
        age: Some(Duration::from_secs(60)),
    }
}

#[test]
fn a_fresh_request_from_the_worktree_lets_the_draft_in() {
    assert_eq!(found().check(), Ok(()));
    let at_the_end = Found {
        age: Some(ASKED_FOR),
        ..found()
    };
    assert_eq!(at_the_end.check(), Ok(()));
}

#[test]
fn another_worktree_an_untrusted_project_a_run_or_an_old_request_is_refused() {
    let other = Found {
        same_worktree: false,
        ..found()
    };
    assert_eq!(
        other.check(),
        Err("that commit message is another worktree's, or asked another session")
    );
    let untrusted = Found {
        trusted: false,
        ..found()
    };
    assert_eq!(untrusted.check(), Err(RESTRICTED));
    let runs = Found {
        runs: true,
        ..found()
    };
    assert!(runs.check().is_err());
    for age in [Some(ASKED_FOR + Duration::from_secs(1)), None] {
        let old = Found { age, ..found() };
        assert!(old.check().is_err());
    }
}
