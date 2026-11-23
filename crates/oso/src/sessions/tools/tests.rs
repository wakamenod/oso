//! The session tools' rules: the table of reach, the loop guard, the person's refusals, the
//! questions' wording, and that `oso mcp` describes each tool as the table says.
//!
//! Must not depend on: the app or threads; the rules are plain data and functions.

use std::time::{Duration, Instant};

use oso_core::command::Reach;

use super::add_repo::AddDir;
use super::{
    Asks, Branch, Guard, MAX_CALLS, REFUSED_FOR, Refusal, Renamable, Resumable, StartLimit, TOOLS,
    Unasked, add_dir, add_repo_question, add_repo_said, addable, quoted, renamable, rename_clash,
    rename_note, renamed_said, resumable, resume_question, resume_started, start_question,
    start_started, starts_without_asking, tool, worktree_question, worktree_started,
};
use crate::sessions::identity::Origin;
use crate::sessions::identity::SessionId;

fn session(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

/// What each tool may do, spelled out: a change to the table shows here in review.
#[test]
fn the_tools_and_their_reach() {
    let table: Vec<_> = TOOLS
        .iter()
        .map(|t| (t.name, t.reach, t.asks, t.counted(), t.outside))
        .collect();
    assert_eq!(
        table,
        [
            ("session_list", Reach::Look, Asks::No, false, true),
            ("session_state", Reach::Look, Asks::No, false, true),
            ("worktree_list", Reach::Look, Asks::No, false, true),
            // Only a hosted session has a space (ADR 0136, step 4).
            ("space_status", Reach::Look, Asks::No, false, false),
            ("worktree_remove", Reach::Change, Asks::No, true, false),
            // They start without asking only as ADR 0135 lets them; nothing else does.
            (
                "start_worktree_session",
                Reach::Change,
                Asks::UnlessAllowed,
                true,
                false
            ),
            (
                "session_start",
                Reach::Change,
                Asks::UnlessAllowed,
                true,
                false
            ),
            // It writes only the caller's worktree's message, for oso's request (ADR 0127).
            ("draft_set", Reach::Change, Asks::No, true, false),
            // The one tool that acts for an outside connection, which asks each time (ADR 0151).
            ("move_to_space", Reach::Change, Asks::Each, true, true),
            // As a start, by ADR 0135's rules (ADR 0153).
            (
                "session_resume",
                Reach::Change,
                Asks::UnlessAllowed,
                true,
                false
            ),
            // Metadata only, as ADR 0121's table has it (ADR 0155).
            ("session_rename", Reach::Change, Asks::No, true, false),
            // As a start, by ADR 0135's rules, past no limit; untrusted, it asks (ADR 0161).
            (
                "space_add_repo",
                Reach::Change,
                Asks::UnlessAllowed,
                true,
                false
            ),
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
        // What changes or asks counts, and only it; only what reads, or asks each time, is
        // open to outsiders.
        assert_eq!(
            t.counted(),
            t.reach == Reach::Change || t.asks == Asks::Each
        );
        assert_eq!(t.read_only(), t.reach == Reach::Look && t.asks == Asks::No);
        assert!(
            !t.outside || t.read_only() || t.asks == Asks::Each,
            "{}",
            t.name
        );
        assert!(!t.destructive || t.counted(), "{}", t.name);
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

/// A resolved target, such as the branch git names, is looked up without counting a call.
#[test]
fn a_refusal_is_seen_by_its_resolved_target_without_a_count() {
    let mut guard = Guard::default();
    let a = session(1);
    let now = Instant::now();
    guard.refused(&a, "start_worktree_session", "main", now);
    let soon = now + Duration::from_secs(30);
    assert!(guard.was_refused(&a, "start_worktree_session", "main", soon));
    assert!(!guard.was_refused(&a, "start_worktree_session", "other", soon));
    assert!(!guard.was_refused(&session(2), "start_worktree_session", "main", soon));
    assert!(!guard.was_refused(&a, "start_worktree_session", "main", now + REFUSED_FOR));
    assert_eq!(guard.calls(&a), 0, "looking counts nothing");
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
            entry["annotations"]["readOnlyHint"],
            t.read_only(),
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

/// Invisible and look-alike characters show as escapes, and a long path keeps its end.
#[test]
fn a_question_shows_what_no_eye_would_see_and_the_end_of_a_long_path() {
    for hidden in [
        "main\u{200b}",
        "m\u{430}in",
        "a\u{feff}",
        "a\u{a0}b",
        "\u{201d}x",
    ] {
        let shown = quoted(hidden);
        assert!(shown.is_ascii(), "{shown}");
        assert!(shown.contains("\\u{"), "{shown}");
    }
    let long = format!("/{}/.claude/worktrees/the-one", "deep/".repeat(40));
    let shown = quoted(&long);
    assert!(shown.ends_with("the-one\""), "{shown}");
    assert!(shown.chars().count() <= 82, "{shown}");
}

#[test]
fn a_start_says_what_started_without_asking() {
    assert_eq!(
        worktree_started(
            "api",
            "/r/.claude/worktrees/x",
            "x",
            Branch::New { from: "main" },
            42,
            "C-c a e"
        ),
        "session \"api\" is starting a session in a new worktree \"/r/.claude/worktrees/x\" on new branch \"x\" from \"main\", without asking (brief: 42 lines; C-c a e explains)"
    );
    assert_eq!(
        start_started("api", "/r", None, "C-c a e"),
        "session \"api\" is starting a session in \"/r\", without asking (no brief; C-c a e explains)"
    );
    assert_eq!(
        start_started("api", "/r", Some(1), "C-c C-e"),
        "session \"api\" is starting a session in \"/r\", without asking (brief: 1 line; C-c C-e explains)"
    );
}

#[test]
fn only_the_person_s_session_starts_without_asking_trusted_and_below_its_limit() {
    let yes = Unasked {
        tool: Asks::UnlessAllowed,
        allowed: true,
        caller: Origin::Person,
        trusted: true,
        live_unasked: 3,
        limit: StartLimit::Most(4),
    };
    assert!(starts_without_asking(yes));
    // `"none"`: no count holds it back (ADR 0144).
    assert!(starts_without_asking(Unasked {
        live_unasked: usize::MAX,
        limit: StartLimit::Unlimited,
        ..yes
    }));
    assert!(!starts_without_asking(Unasked {
        limit: StartLimit::Unlimited,
        trusted: false,
        ..yes
    }));
    assert!(starts_without_asking(Unasked {
        caller: Origin::Answered,
        ..yes
    }));
    for no in [
        Unasked {
            allowed: false,
            ..yes
        },
        Unasked {
            tool: Asks::Each,
            ..yes
        },
        Unasked {
            caller: Origin::Unasked,
            ..yes
        },
        Unasked {
            caller: Origin::Unknown,
            ..yes
        },
        Unasked {
            trusted: false,
            ..yes
        },
        Unasked {
            live_unasked: 4,
            ..yes
        },
        Unasked {
            live_unasked: 9,
            ..yes
        },
    ] {
        assert!(!starts_without_asking(no), "{no:?}");
    }
}

#[test]
fn a_question_names_each_directory_the_new_session_also_reads() {
    let said = super::also_reads("let it start?".to_owned(), &[]);
    assert_eq!(said, "let it start?");
    let dirs = [
        std::path::PathBuf::from("/web"),
        std::path::PathBuf::from("/a\u{202e}b"),
    ];
    let said = super::also_reads("let it start?".to_owned(), &dirs);
    assert_eq!(
        said,
        "let it start? It also reads 2 more directories: \"/web\", \"/a\\u{202e}b\"."
    );
}

/// A resume takes a stopped session of the space with its buffer, and refuses the caller, one
/// that runs, an archived one and an orchestrator (ADR 0153).
#[test]
fn only_a_stopped_session_with_its_buffer_is_resumed() {
    let stopped = Resumable {
        buffer: true,
        ..Resumable::default()
    };
    assert_eq!(resumable(stopped), Ok(()));
    let refused = [
        Resumable {
            caller: true,
            ..stopped
        },
        Resumable {
            runs: true,
            ..stopped
        },
        Resumable {
            archived: true,
            ..stopped
        },
        Resumable {
            orchestrator: true,
            ..stopped
        },
        Resumable {
            buffer: false,
            ..stopped
        },
    ];
    for session in refused {
        assert!(resumable(session).is_err(), "{session:?}");
    }
}

/// The resume's question and its line quote what an agent or a repository named.
#[test]
fn a_resume_names_both_sessions_and_the_directory_quoted() {
    assert_eq!(
        resume_question("fix-login", "api", "/r/.claude/worktrees/api"),
        "let session \"fix-login\" resume session \"api\" in \"/r/.claude/worktrees/api\"?"
    );
    let said = resume_started("fix-login", "a\u{202e}b", "/r");
    assert!(said.contains("\\u{202e}"), "{said}");
    assert!(said.ends_with(", without asking"), "{said}");
}

/// A resume goes without asking by the same rule as a start, the resumed sessions counted
/// toward the limit (ADR 0153).
#[test]
fn a_resume_goes_without_asking_as_a_start_does() {
    let resume = tool("session_resume").unwrap();
    let unasked = |live_unasked| {
        starts_without_asking(Unasked {
            tool: resume.asks,
            allowed: true,
            caller: Origin::Person,
            trusted: true,
            live_unasked,
            limit: StartLimit::Most(4),
        })
    };
    assert!(unasked(3));
    assert!(!unasked(4), "past the limit it asks");
}

/// The person renames any session but a current orchestrator; an agent renames itself, its
/// children and, as a current orchestrator, its space's sessions, never an orchestrator
/// (ADR 0155).
#[test]
fn who_renames_which_session() {
    let person = Renamable::default();
    assert_eq!(renamable(person), Ok(()));
    assert_eq!(
        renamable(Renamable {
            former: true,
            ..person
        }),
        Ok(())
    );
    assert!(
        renamable(Renamable {
            orchestrator: true,
            ..person
        })
        .is_err()
    );
    let agent = Renamable {
        agent: true,
        ..Renamable::default()
    };
    assert!(renamable(agent).is_err(), "another's session");
    for allowed in [
        Renamable {
            caller: true,
            ..agent
        },
        Renamable {
            callers_child: true,
            ..agent
        },
        Renamable {
            of_callers_space: true,
            ..agent
        },
    ] {
        assert_eq!(renamable(allowed), Ok(()), "{allowed:?}");
        for orchestrator in [
            Renamable {
                orchestrator: true,
                ..allowed
            },
            Renamable {
                former: true,
                ..allowed
            },
        ] {
            assert!(renamable(orchestrator).is_err(), "{orchestrator:?}");
        }
    }
}

/// A rename takes no name another session, a space or a space's earlier orchestrator has,
/// in any case (ADR 0155).
#[test]
fn a_rename_takes_no_name_held_or_answered_to() {
    let sessions = ["api", "web-2"];
    let spaces = ["fix-login"];
    let clash = |name| rename_clash(name, sessions, spaces);
    assert_eq!(clash("new"), None);
    assert!(clash("API").unwrap().contains("another session"));
    assert!(clash("Fix-Login").unwrap().contains("a space"));
    assert!(clash("fix-login-was-2").unwrap().contains("earlier"));
    assert_eq!(clash("fix-login-2"), None);
}

/// The rename's line names both names quoted, and its note what still answers to the old one.
#[test]
fn a_rename_says_what_still_answers_to_the_old_name() {
    assert_eq!(
        renamed_said(Some("lead"), "api", "a\u{202e}b"),
        "session \"lead\" renamed session \"api\" to \"a\\u{202e}b\""
    );
    assert_eq!(
        renamed_said(None, "api", "api-v2"),
        "Session \"api\" is now \"api-v2\""
    );
    let note = rename_note("api", "api-v2", true, 0).unwrap();
    assert!(note.contains("answers to \"api\""), "{note}");
    assert!(note.contains("/rename api-v2"), "{note}");
    assert_eq!(rename_note("api", "api-v2", false, 0), None);
    // Claude Code took the name already.
    assert_eq!(rename_note("api-v2", "api-v2", true, 0), None);
    let children = rename_note("api", "api-v2", false, 2).unwrap();
    assert_eq!(children, "2 sessions it started report to \"api\"");
}

/// The add's question quotes the path and the space, and says when the repository joins
/// restricted; its line says it went without asking (ADR 0161).
#[test]
fn an_add_names_the_repository_quoted_and_says_when_it_is_restricted() {
    let trusted = add_repo_question("fix-login", "/r/web", "fix-login", true);
    assert_eq!(
        trusted,
        "let session \"fix-login\" add repository \"/r/web\" to space \"fix-login\"?"
    );
    let restricted = add_repo_question("fix-login", "/r/w\u{202e}eb", "fix-login", false);
    assert!(restricted.contains("\\u{202e}"), "{restricted}");
    assert!(restricted.contains("joins restricted"), "{restricted}");
    let said = add_repo_said("fix-login", "/r/web", "fix-login", true);
    assert!(said.ends_with(", without asking"), "{said}");
    let said = add_repo_said("fix-login", "/r/web", "fix-login", false);
    assert!(said.ends_with("to space \"fix-login\""), "{said}");
}

/// An add goes without asking only as a start would, trusted, and counts toward no limit, as
/// it starts no session (ADR 0161, Q2, Q4).
#[test]
fn an_add_goes_without_asking_only_when_trusted_and_past_no_limit() {
    let add = tool("space_add_repo").unwrap();
    let unasked = |trusted, caller| {
        starts_without_asking(Unasked {
            tool: add.asks,
            allowed: true,
            caller,
            trusted,
            live_unasked: 0,
            limit: StartLimit::Unlimited,
        })
    };
    assert!(unasked(true, Origin::Person));
    assert!(!unasked(false, Origin::Person), "untrusted, it always asks");
    assert!(!unasked(true, Origin::Unasked), "one level only");
    let starting: Vec<_> = TOOLS.iter().filter(|t| t.starts).map(|t| t.name).collect();
    assert_eq!(
        starting,
        ["start_worktree_session", "session_start", "session_resume"],
        "only these count toward agents_start_limit"
    );
}

/// A space takes a repository only while open, once, and up to 16 (ADR 0161).
#[test]
fn a_space_takes_a_repository_open_once_and_up_to_sixteen() {
    assert!(addable(true, false, 15).is_ok());
    assert!(addable(false, false, 0).unwrap_err().contains("closed"));
    assert!(addable(true, true, 1).unwrap_err().contains("already"));
    assert!(addable(true, false, 16).unwrap_err().contains("at most 16"));
}

/// The reply says the agent cannot read a repository until the person's paste, and a
/// restricted one until their trust (ADR 0161, Q5).
#[test]
fn the_reply_says_what_reads_the_added_repository() {
    assert_eq!(add_dir(false, true, true, false), AddDir::WaitsForTrust);
    assert_eq!(add_dir(true, true, false, false), AddDir::Given);
    assert_eq!(add_dir(true, false, true, false), AddDir::Offered);
    assert_eq!(add_dir(true, false, false, false), AddDir::NotOffered);
    let native = add_dir(true, false, true, true);
    assert_eq!(native, AddDir::NextResume);
    let note = AddDir::Offered.note("/r/web");
    assert!(
        note.contains("/add-dir /r/web") && note.contains("ask them"),
        "{note}"
    );
    assert!(
        AddDir::WaitsForTrust
            .note("/r/web")
            .contains("trust-project")
    );
    assert_eq!(AddDir::NotOffered.word(), "not offered");
}
