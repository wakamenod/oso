//! The rules of an agent's `session_archive` (ADR 0179): whom it archives, when one that runs
//! is stopped first, what the registry must say, and what is said.
//!
//! Must not depend on: the app or threads; the rules are plain data and functions.

use super::{
    Archivable, Archive, NotIdle, Runs, archivable, archive_asks, archive_question, archived_said,
    registry_idle, stops_without_asking,
};
use crate::sessions::identity::Origin;
use crate::sessions::registry::{Entry, Live};
use crate::sessions::tools::{Asks, CallerStanding, tool};

/// A stopped session of the caller's space, which its orchestrator may archive.
fn of_space() -> Archivable {
    Archivable {
        of_callers_space: true,
        ..Archivable::default()
    }
}

#[test]
fn a_stopped_session_of_the_space_or_the_callers_own_is_archived_as_a_row() {
    assert_eq!(archivable(of_space()), Ok(Archive::Row));
    let child = Archivable {
        callers_child: true,
        ..Archivable::default()
    };
    assert_eq!(archivable(child), Ok(Archive::Row));
}

#[test]
fn the_caller_orchestrators_pinned_archived_and_strangers_are_refused() {
    let refused = |session: Archivable| archivable(session).unwrap_err();
    let caller = Archivable {
        caller: true,
        ..of_space()
    };
    assert!(refused(caller).contains("your own"));
    let orchestrator = Archivable {
        orchestrator: true,
        ..of_space()
    };
    assert!(refused(orchestrator).contains("orchestrator"));
    let pinned = Archivable {
        pinned: true,
        ..of_space()
    };
    assert!(refused(pinned).contains("pinned"));
    let archived = Archivable {
        archived: true,
        ..of_space()
    };
    assert!(refused(archived).contains("archived already"));
    // Neither of the caller's space nor started by it.
    assert!(refused(Archivable::default()).contains("sessions it started"));
    // Its own session is refused before the scope is looked at.
    let own = Archivable {
        caller: true,
        ..Archivable::default()
    };
    assert!(refused(own).contains("your own"));
}

#[test]
fn one_that_runs_is_stopped_first_only_when_settled_with_a_known_process() {
    let runs = |runs: Runs| archivable(Archivable { runs, ..of_space() });
    assert_eq!(
        runs(Runs::Yes {
            pid: Some(7),
            settled: true
        }),
        Ok(Archive::StopFirst { pid: 7 })
    );
    let working = runs(Runs::Yes {
        pid: Some(7),
        settled: false,
    });
    assert!(working.unwrap_err().contains("may still work"));
    let unknown = runs(Runs::Yes {
        pid: None,
        settled: true,
    });
    assert!(unknown.unwrap_err().contains("process"));
    assert!(runs(Runs::Starting).unwrap_err().contains("starting"));
    assert!(runs(Runs::Stopping).unwrap_err().contains("stopping"));
}

fn entry(pid: u32, status: Option<&str>) -> Entry {
    Entry {
        pid,
        status: status.map(str::to_owned),
        ..Entry::default()
    }
}

#[test]
fn only_a_live_entry_of_its_own_process_that_says_idle_is_idle() {
    let idle = entry(7, Some("idle"));
    assert_eq!(registry_idle(7, Some((&idle, Live::Yes))), Ok(()));
    assert_eq!(registry_idle(7, None), Err(NotIdle::NoEntry));
    assert_eq!(
        registry_idle(8, Some((&idle, Live::Yes))),
        Err(NotIdle::NoEntry),
        "another process's entry"
    );
    assert_eq!(
        registry_idle(7, Some((&idle, Live::Unknown))),
        Err(NotIdle::Unverified)
    );
    assert_eq!(
        registry_idle(7, Some((&idle, Live::No))),
        Err(NotIdle::Unverified)
    );
    let shell = entry(7, Some("shell"));
    assert_eq!(
        registry_idle(7, Some((&shell, Live::Yes))),
        Err(NotIdle::Shell)
    );
    assert!(NotIdle::Shell.why().contains("background shell"));
    for status in [Some("busy"), Some("IDLE"), None] {
        let other = entry(7, status);
        assert_eq!(
            registry_idle(7, Some((&other, Live::Yes))),
            Err(NotIdle::Busy),
            "{status:?}"
        );
    }
}

#[test]
fn the_question_and_the_echo_quote_the_names_and_say_the_stop() {
    assert_eq!(
        archive_question("lead", "w\"1", "/r/w"),
        "let session \"lead\" stop idle session \"w\\\"1\" in \"/r/w\" and archive it? Its worktree is kept."
    );
    assert_eq!(
        archived_said("lead", "w", false, false),
        "session \"lead\" archived session \"w\"; A under Archived in *Spaces* brings it back"
    );
    assert!(archived_said("lead", "w", true, false).contains(", stopping it first;"));
    assert!(archived_said("lead", "w", true, true).contains("first without asking;"));
}

/// The row alone asks nothing; a stop asks as the table says of the tool.
#[test]
fn only_a_stop_asks_as_the_table_says() {
    let archive = tool("session_archive").unwrap();
    assert_eq!(archive.asks, Asks::UnlessAllowed);
    assert_eq!(archive_asks(Archive::Row, archive), Asks::No);
    assert_eq!(
        archive_asks(Archive::StopFirst { pid: 7 }, archive),
        Asks::UnlessAllowed
    );
}

/// A stop goes without asking only as the setting allows, from a session the person started:
/// one started without asking asks, as its own starts do.
#[test]
fn a_stop_goes_unasked_only_for_the_persons_caller_with_the_setting_on() {
    let stops = |allowed, caller| {
        stops_without_asking(CallerStanding {
            allowed,
            caller,
            connected: false,
        })
    };
    assert!(stops(true, Origin::Person));
    assert!(stops(true, Origin::Answered));
    assert!(!stops(true, Origin::Unasked));
    assert!(!stops(true, Origin::Unknown));
    assert!(!stops(false, Origin::Person));
}
