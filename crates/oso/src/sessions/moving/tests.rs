//! Tests of moving a session started outside oso (ADR 0151) on every system: the wait's
//! steps, the judging of a move, the bridge's wait, and the words. Those that read a registry
//! and the process table are in `looking`, on Unix.
//!
//! Must not depend on: `app`, the system's own registry, or the disk.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{
    Asker, BridgeWait, GIVE_UP, Held, LATE, NOT_WAITING, Recorded, Settling, Step, TOO_LATE,
    Watched, called, details, gave_up, idle, judge, question, running, step,
};
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::Member;

const ID: &str = "1234abcd-0000-4000-8000-00000000aaaa";
const PID: u32 = 4242;

fn started() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_794_512_815)
}

/// The process table as it is when [`PID`] still runs from [`started`].
fn still(pid: u32) -> io::Result<Option<SystemTime>> {
    Ok((pid == PID).then(started))
}

#[test]
fn the_wait_stops_once_the_process_ended_and_gives_up_after_five_minutes() {
    let soon = Duration::from_secs(1);
    assert_eq!(step(soon, true, false), Step::Wait);
    assert_eq!(step(soon, false, false), Step::Stopped);
    assert_eq!(step(LATE, true, false), Step::Late);
    assert_eq!(step(LATE, true, true), Step::Wait);
    assert_eq!(step(LATE, false, true), Step::Stopped);
    assert_eq!(step(GIVE_UP, true, true), Step::GiveUp);
    assert_eq!(step(GIVE_UP, false, true), Step::Stopped);
    // The process with the id must be the one that started then.
    assert!(running(PID, started(), still));
    assert!(!running(PID, started(), |_| Ok(None)));
    let other = |_| Ok(Some(started() + Duration::from_secs(30)));
    assert!(!running(PID, started(), other));
    // Linux counts a start from boot, which moves when the clock is set: within the
    // registry's slack it is the same process, or a resume could fork the conversation.
    let stepped = |_| Ok(Some(started() + Duration::from_secs(1)));
    assert!(running(PID, started(), stepped));
    let back = |_| Ok(Some(started() - Duration::from_secs(1)));
    assert!(running(PID, started(), back));
    // A system that cannot tell keeps the wait going: a resume could fork the conversation.
    assert!(running(PID, started(), |_| Err(io::Error::other("no"))));
}

fn asker(directory: &str) -> Asker {
    Asker {
        pid: PID,
        started: started(),
        id: SessionId::parse(ID).unwrap(),
        program: Some("claude".into()),
        name: Some("api-fix".into()),
        directory: PathBuf::from(directory),
        main: Member {
            root: PathBuf::from("/w/api"),
            identity: None,
        },
        worktree: None,
    }
}

#[test]
fn the_question_names_the_session_its_directory_and_what_is_added() {
    let asking = asker("/w/api/src");
    let id = SessionId::parse(ID).unwrap();
    assert_eq!(called(Some("api-fix"), &id), "api-fix");
    assert_eq!(called(Some("  "), &id), "1234abcd");
    assert_eq!(called(None, &id), "1234abcd");
    let label = question("api-fix", &asking, "fix-login", None);
    assert_eq!(
        label,
        "move session \"api-fix\" (\"claude\", pid 4242, in \"/w/api/src\", outside oso) into space \
         \"fix-login\"? It stops there first, then resumes here with your settings and may \
         start sessions as yours do."
    );
    let adding = question("api-fix", &asking, "fix-login", Some(Path::new("/w/api")));
    assert!(
        adding.contains("into space \"fix-login\", adding repository \"/w/api\" to it?"),
        "{adding}"
    );
    let shown = details(&label, &asking, "fix-login", None);
    assert!(shown.contains(ID), "{shown}");
    assert!(shown.contains("/exit"), "{shown}");
    // The question names the process as the system does, not as the registry says.
    let mut shell = asker("/w/api/src");
    shell.program = Some("sh".into());
    let label = question("api-fix", &shell, "fix-login", None);
    assert!(label.contains("(\"sh\", pid 4242"), "{label}");
    shell.program = None;
    let label = question("api-fix", &shell, "fix-login", None);
    assert!(label.contains("(a process with no name, pid"), "{label}");
    // `?` shows a long path whole, and says which space a worktree leaves.
    let long = format!("/w/{}/api", "d".repeat(200));
    let mut deep = asker(&long);
    deep.worktree = Some(PathBuf::from(&long));
    let shown = details(&label, &deep, "fix-login", Some("other"));
    assert!(shown.contains(&long), "{shown}");
    assert!(shown.contains("which leaves space \"other\""), "{shown}");
}

#[test]
fn registry_text_is_escaped_in_every_word() {
    // A name and a directory from the registry, which any process of the person's may write.
    let asking = asker("/w/a\u{202e}pi\n");
    let label = question("x\"\u{1b}[2J", &asking, "s", None);
    assert!(!label.contains('\u{202e}') && !label.contains('\n') && !label.contains('\u{1b}'));
    assert!(label.contains("\\u{202e}"), "{label}");
    let said = gave_up("\u{202e}name");
    assert!(!said.contains('\u{202e}'), "{said}");
}

#[test]
fn a_move_is_judged_by_the_record_the_trust_and_the_space() {
    let asking = asker("/w/api/src");
    let held = |record, trusted, member, members| Held {
        record,
        trusted,
        member,
        members,
    };
    // A new repository joins the space; one it has does not.
    assert_eq!(judge(&asking, "s", &held(None, true, false, 1)), Ok(true));
    assert_eq!(judge(&asking, "s", &held(None, true, true, 16)), Ok(false));
    // A full space, an untrusted repository.
    let full = judge(&asking, "s", &held(None, true, false, 16)).unwrap_err();
    assert!(full.contains("16 repositories"), "{full}");
    let untrusted = judge(&asking, "s", &held(None, false, true, 1)).unwrap_err();
    assert!(untrusted.contains("trust-project"), "{untrusted}");
    // oso's record: a running one forks, another directory or an orchestrator does not move,
    // and a stopped one in the same directory does.
    let recorded = |runs, directory: &'static str, orchestrates| Recorded {
        name: "api-fix",
        runs,
        directory: Path::new(directory),
        orchestrates,
    };
    let running = held(Some(recorded(true, "/w/api/src", false)), true, true, 1);
    assert!(
        judge(&asking, "s", &running)
            .unwrap_err()
            .contains("runs this")
    );
    let elsewhere = held(Some(recorded(false, "/w/other", false)), true, true, 1);
    assert!(
        judge(&asking, "s", &elsewhere)
            .unwrap_err()
            .contains("another directory")
    );
    let orchestrator = held(Some(recorded(false, "/w/api/src", true)), true, true, 1);
    assert!(
        judge(&asking, "s", &orchestrator)
            .unwrap_err()
            .contains("orchestrator")
    );
    let stopped = held(Some(recorded(false, "/w/api/src", false)), true, false, 1);
    assert_eq!(judge(&asking, "s", &stopped), Ok(true));
}

#[test]
fn a_space_name_is_as_long_as_the_channel_takes() {
    assert_eq!(
        crate::channel::protocol::sessions::MAX_SPACE_NAME,
        crate::sessions::spaces::stored::MAX_NAME
    );
}

#[test]
fn a_turn_counts_as_ended_only_once_the_entry_says_so_after_the_reply() {
    let watched = Watched {
        pid: PID,
        then: started(),
        id: SessionId::parse(ID).unwrap(),
        since_ms: 1_000,
    };
    let entry = |status: &str, at: Option<u64>| crate::sessions::registry::Entry {
        pid: PID,
        session_id: SessionId::parse(ID),
        status: Some(status.into()),
        status_updated_at: at,
        ..Default::default()
    };
    assert_eq!(
        idle(Some(&entry("idle", Some(1_001))), &watched),
        Some(1_001)
    );
    // Busy still; idle from before the reply; idle with no time it changed.
    assert_eq!(idle(Some(&entry("busy", Some(2_000))), &watched), None);
    assert_eq!(idle(Some(&entry("idle", Some(1_000))), &watched), None);
    assert_eq!(idle(Some(&entry("idle", None)), &watched), None);
    assert_eq!(idle(None, &watched), None);
    // Another conversation's entry, or another process's.
    let mut other = entry("idle", Some(2_000));
    other.session_id = SessionId::parse("1234abcd-0000-4000-8000-00000000bbbb");
    assert_eq!(idle(Some(&other), &watched), None);
    let mut moved = entry("idle", Some(2_000));
    moved.pid = PID + 1;
    assert_eq!(idle(Some(&moved), &watched), None);
}

#[test]
fn a_turn_ends_on_the_same_idle_read_twice() {
    let mut settling = Settling::default();
    assert!(!settling.saw(Some(5)));
    assert!(settling.saw(Some(5)));
    // An idle between two turns: busy again at the next reading, then a later idle.
    let mut settling = Settling::default();
    assert!(!settling.saw(Some(5)));
    assert!(!settling.saw(None));
    assert!(!settling.saw(Some(9)));
    assert!(settling.saw(Some(9)));
    assert!(!Settling::default().saw(None));
}

#[test]
fn the_bridges_wait_is_held_once_and_answered_once() {
    // Held, then answered when the turn ends; asked again, refused.
    let mut wait = BridgeWait::default();
    assert!(wait.ask(1).is_ok());
    assert_eq!(wait.ask(2), Err((2, NOT_WAITING)));
    assert_eq!(wait.turn_ended(), Some(1));
    assert_eq!(wait.turn_ended(), None);
    assert_eq!(wait.ask(3), Err((3, NOT_WAITING)));
    // A turn that ended before it was asked answers nothing later.
    let mut early = BridgeWait::default();
    assert_eq!(early.turn_ended(), None);
    assert_eq!(early.ask(4), Err((4, NOT_WAITING)));
    // At 120 s a held one is refused, and one asked later is too.
    let mut late = BridgeWait::default();
    assert!(late.ask(5).is_ok());
    assert_eq!(late.late(), Some(5));
    assert_eq!(late.turn_ended(), None);
    let mut later = BridgeWait::<u8>::default();
    assert_eq!(later.late(), None);
    assert_eq!(later.ask(6), Err((6, TOO_LATE)));
    // Any other end refuses the held one.
    let mut ended = BridgeWait::default();
    assert!(ended.ask(7).is_ok());
    assert_eq!(ended.ended(), Some(7));
}

#[test]
fn the_channel_holds_the_bridges_wait_past_the_point_it_is_answered_by() {
    assert!(LATE < crate::channel::protocol::MOVE_WAIT_WITHIN);
}

#[cfg(unix)]
mod looking;
