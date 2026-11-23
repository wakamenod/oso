//! The guard's rules: the person's 60 s refusal and the rate of calls that ask or change
//! (ADR 0121, step 6; ADR 0178).
//!
//! Must not depend on: the app or threads; the guard takes the time it is given.

use std::time::{Duration, Instant};

use super::{CALLS_A_MINUTE, Guard, RATE_WINDOW, REFUSED_FOR, Refusal};
use crate::sessions::identity::SessionId;

fn session(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

/// A session makes at most [`CALLS_A_MINUTE`] calls in any minute, whatever their tools and
/// targets, and is told how long to wait (ADR 0178); another session has its own.
#[test]
fn a_session_makes_at_most_thirty_calls_a_minute() {
    let mut guard = Guard::default();
    let (a, b) = (session(1), session(2));
    let now = Instant::now();
    for n in 0..CALLS_A_MINUTE {
        let tool = if n % 2 == 0 {
            "session_start"
        } else {
            "worktree_remove"
        };
        let at = now + Duration::from_secs(n as u64);
        assert_eq!(guard.admit(&a, tool, &n.to_string(), at), Ok(()));
    }
    let last = now + Duration::from_secs(CALLS_A_MINUTE as u64 - 1);
    assert_eq!(
        guard.admit(&a, "session_rename", "x", last),
        Err(Refusal::Rate { wait_secs: 31 })
    );
    assert_eq!(guard.admit(&b, "session_rename", "x", last), Ok(()));
    // A refused call does not count: the first call's place frees a minute after it came.
    let half = now + RATE_WINDOW - Duration::from_millis(500);
    assert_eq!(
        guard.admit(&a, "session_rename", "x", half),
        Err(Refusal::Rate { wait_secs: 1 })
    );
    assert_eq!(
        guard.admit(&a, "session_rename", "x", now + RATE_WINDOW),
        Ok(())
    );
    assert_eq!(
        guard.admit(&a, "session_rename", "x", now + RATE_WINDOW),
        Err(Refusal::Rate { wait_secs: 1 })
    );
    let why = Refusal::Rate { wait_secs: 17 }.why();
    assert!(why.contains("wait 17 s and try again"), "{why}");
    assert!(why.contains("do not reach its effect another way"), "{why}");
}

/// The person's refusal is said before the rate, and a call it refuses does not count.
#[test]
fn the_persons_refusal_comes_first_and_does_not_count() {
    let mut guard = Guard::default();
    let a = session(1);
    let now = Instant::now();
    guard.refused(&a, "session_start", "same", now);
    for _ in 0..100 {
        assert_eq!(
            guard.admit(&a, "session_start", "same", now),
            Err(Refusal::Person)
        );
    }
    for n in 0..CALLS_A_MINUTE {
        assert_eq!(
            guard.admit(&a, "session_start", &n.to_string(), now),
            Ok(())
        );
    }
    assert_eq!(
        guard.admit(&a, "session_start", "same", now),
        Err(Refusal::Person)
    );
    assert!(matches!(
        guard.admit(&a, "session_start", "other", now),
        Err(Refusal::Rate { .. })
    ));
}

/// The sessions whose calls are kept stay bounded: those with none in the window go.
#[test]
fn rated_sessions_are_bounded() {
    let mut guard = Guard::default();
    let now = Instant::now();
    for n in 0..=255 {
        assert_eq!(guard.admit(&session(n), "session_start", "x", now), Ok(()));
    }
    let later = now + RATE_WINDOW;
    let another = SessionId::from_random([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    assert_eq!(guard.admit(&another, "session_start", "x", later), Ok(()));
    assert_eq!(guard.recent.len(), 1);
}

#[test]
fn a_refusal_holds_the_same_call_from_the_same_session_for_a_minute() {
    let mut guard = Guard::default();
    let (a, b) = (session(1), session(2));
    let now = Instant::now();
    guard.refused(&a, "start_worktree_session", "x", now);
    let soon = now + Duration::from_secs(59);
    assert_eq!(
        guard.admit(&a, "start_worktree_session", "x", soon),
        Err(Refusal::Person)
    );
    // Another target, tool or session asks again.
    assert_eq!(guard.admit(&a, "start_worktree_session", "y", soon), Ok(()));
    assert_eq!(guard.admit(&a, "session_start", "x", soon), Ok(()));
    assert_eq!(guard.admit(&b, "start_worktree_session", "x", soon), Ok(()));
    assert_eq!(
        guard.admit(&a, "start_worktree_session", "x", now + REFUSED_FOR),
        Ok(())
    );
}

/// A resolved target, such as the branch git names, is looked up.
#[test]
fn a_refusal_is_seen_by_its_resolved_target() {
    let mut guard = Guard::default();
    let a = session(1);
    let now = Instant::now();
    guard.refused(&a, "start_worktree_session", "main", now);
    let soon = now + Duration::from_secs(30);
    assert!(guard.was_refused(&a, "start_worktree_session", "main", soon));
    assert!(!guard.was_refused(&a, "start_worktree_session", "other", soon));
    assert!(!guard.was_refused(&session(2), "start_worktree_session", "main", soon));
    assert!(!guard.was_refused(&a, "start_worktree_session", "main", now + REFUSED_FOR));
}

#[test]
fn refusals_are_bounded_and_the_oldest_goes() {
    let mut guard = Guard::default();
    let a = session(1);
    let now = Instant::now();
    for n in 0..300 {
        guard.refused(
            &a,
            "session_start",
            &n.to_string(),
            now + Duration::from_millis(n),
        );
    }
    let later = now + Duration::from_millis(400);
    assert_eq!(
        guard.admit(&a, "session_start", "299", later),
        Err(Refusal::Person)
    );
    assert_eq!(guard.admit(&a, "session_start", "0", later), Ok(()));
}

/// A session at its limit keeps its window when many others come: only those with no call in
/// the window go.
#[test]
fn a_full_map_never_frees_an_active_session() {
    let mut guard = Guard::default();
    let now = Instant::now();
    let a = SessionId::from_random([9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6]);
    for n in 0..CALLS_A_MINUTE {
        assert_eq!(
            guard.admit(&a, "session_start", &n.to_string(), now),
            Ok(())
        );
    }
    for n in 0..=255 {
        assert_eq!(guard.admit(&session(n), "session_start", "x", now), Ok(()));
    }
    let another = SessionId::from_random([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    let soon = now + Duration::from_secs(1);
    assert_eq!(guard.admit(&another, "session_start", "x", soon), Ok(()));
    assert!(matches!(
        guard.admit(&a, "session_start", "y", soon),
        Err(Refusal::Rate { wait_secs: 59 })
    ));
}
