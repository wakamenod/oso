//! The session tools' rules: the table of reach, the loop guard, the person's refusals, the
//! questions' wording, and that `oso mcp` describes each tool as the table says.
//!
//! Must not depend on: the app or threads; the rules are plain data and functions.

use std::time::{Duration, Instant};

use oso_core::command::Reach;

use super::{
    Asks, Branch, Guard, MAX_CALLS, REFUSED_FOR, Refusal, TOOLS, quoted, start_question, tool,
    worktree_question,
};
use crate::sessions::identity::SessionId;

fn session(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

/// What each tool may do, spelled out: a change to the table shows here in review.
#[test]
fn the_tools_and_their_reach() {
    let table: Vec<_> = TOOLS
        .iter()
        .map(|t| (t.name, t.reach, t.asks, t.counted, t.outside))
        .collect();
    assert_eq!(
        table,
        [
            ("session_list", Reach::Look, Asks::No, false, true),
            ("session_state", Reach::Look, Asks::No, false, true),
            ("worktree_list", Reach::Look, Asks::No, false, true),
            ("worktree_remove", Reach::Change, Asks::No, true, false),
            (
                "start_worktree_session",
                Reach::Change,
                Asks::Each,
                true,
                false
            ),
            ("session_start", Reach::Change, Asks::Each, true, false),
        ]
    );
    let destructive: Vec<_> = TOOLS
        .iter()
        .filter(|t| t.destructive)
        .map(|t| t.name)
        .collect();
    assert_eq!(destructive, ["worktree_remove"]);
}

/// The table's own rules, whatever it holds.
#[test]
fn every_tool_keeps_the_rules_of_reach() {
    for t in &TOOLS {
        // No tool is the person's: those never reach the channel.
        assert_ne!(t.reach, Reach::Person, "{}", t.name);
        // What changes or asks counts, and only it; only what reads is open to outsiders.
        assert_eq!(t.counted, t.reach == Reach::Change || t.asks == Asks::Each);
        assert_eq!(t.read_only, t.reach == Reach::Look && t.asks == Asks::No);
        assert!(!t.outside || t.read_only, "{}", t.name);
        assert_eq!(tool(t.name), Some(t));
    }
    assert!(tool("state").is_none());
}

#[test]
fn the_loop_guard_refuses_the_call_past_the_last_and_typing_resets_it() {
    let mut guard = Guard::default();
    let (a, b) = (session(1), session(2));
    let now = Instant::now();
    for n in 0..MAX_CALLS {
        assert_eq!(
            guard.admit(&a, "session_start", &n.to_string(), now),
            Ok(())
        );
    }
    assert_eq!(
        guard.admit(&a, "session_start", "more", now),
        Err(Refusal::Loop)
    );
    assert_eq!(guard.calls(&a), MAX_CALLS, "a refused call is not counted");
    // Another session has its own count.
    assert_eq!(guard.admit(&b, "session_start", "x", now), Ok(()));
    guard.person_typed(&a);
    assert_eq!(guard.admit(&a, "session_start", "more", now), Ok(()));
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

#[test]
fn a_question_names_what_starts_and_no_name_rewrites_it() {
    assert_eq!(
        worktree_question(
            "api",
            "/r/.claude/worktrees/x",
            "x",
            Branch::New { from: "main" },
            42
        ),
        "let session \"api\" start a session in a new worktree \"/r/.claude/worktrees/x\" on new branch \"x\" from \"main\", with a brief of 42 lines (? to read it)?"
    );
    assert_eq!(
        start_question("api", "/r", None, true),
        "let session \"api\" start a session in \"/r\"? Another session runs there."
    );
    assert_eq!(
        start_question("api", "/r", Some(1), false),
        "let session \"api\" start a session in \"/r\", with a brief of 1 line (? to read it)?"
    );
    let forged = quoted("x\" from \"main\", with a brief of 1 line?\u{202e}\n");
    assert!(forged.starts_with("\"x\\\" from"), "{forged}");
    assert!(
        !forged.contains('\n') && !forged.contains('\u{202e}'),
        "{forged}"
    );
    // Its own three quotes, each escaped, between the two that quote it.
    assert_eq!(forged.matches('"').count(), 2 + 3, "{forged}");
    assert!(quoted(&"a".repeat(500)).chars().count() < 90);
}

/// What `oso mcp` says of each session tool agrees with the table, and every session tool it
/// lists is in the table, and the channel's method of the same name.
#[test]
fn the_bridge_lists_each_tool_with_its_hints() {
    let listed = crate::channel::mcp::listed_tools();
    let listed = listed.as_array().unwrap();
    let methods = crate::channel::protocol::sessions::METHODS;
    for t in &TOOLS {
        let entry = listed
            .iter()
            .find(|entry| entry["name"] == t.name)
            .unwrap_or_else(|| panic!("{} is not listed", t.name));
        assert_eq!(
            entry["annotations"]["readOnlyHint"], t.read_only,
            "{}",
            t.name
        );
        assert_eq!(
            entry["annotations"]["destructiveHint"], t.destructive,
            "{}",
            t.name
        );
        assert!(methods.contains(&t.name), "{}", t.name);
    }
    assert_eq!(methods.len(), TOOLS.len());
}
