//! When oso asks to connect, what it comes back to after a restart, what it says, and a start
//! blocked after all (ADR 0181).
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::time::Instant;

use super::{MAX_ROWS, Phase, Remote};
use crate::sessions::native::control::Control;
use crate::sessions::native::control::tests::{on, refused};
use crate::sessions::native::wire::control::INITIALIZE;
use crate::sessions::native::wire::remote::{Enabled, RemoteId};
use crate::sessions::native::wire::{Event, parse};

fn enabled(id: &str) -> Enabled {
    let line = format!(
        r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"oso-1","response":{{"bridge_session_id":"{id}"}}}}}}"#
    );
    match parse(line.as_bytes()) {
        Event::Answer(answer) => answer.remote.map(|e| *e).unwrap_or_default(),
        other => panic!("{other:?}"),
    }
}

fn id(text: &str) -> RemoteId {
    enabled(text).id.unwrap()
}

fn bridge(state: &str, why: &str) -> Event {
    let line = serde_json::json!({
        "type": "system", "subtype": "bridge_state", "state": state, "detail": why,
    });
    parse(line.to_string().as_bytes())
}

fn seven() -> std::io::Result<u64> {
    Ok(7)
}

#[test]
fn it_asks_once_where_initialize_said_it_is_available() {
    let now = Instant::now();
    let mut remote = Remote::unblocked();
    let mut starting = Control::started(now);
    assert_eq!(
        remote.ready(&mut starting, seven, now),
        None,
        "not known yet"
    );
    assert_eq!(remote.phase(), &Phase::Connecting);
    let mut control = on(now);
    remote.initialized(Some(true));
    assert!(remote.ready(&mut control, seven, now).is_some());
    assert_eq!(remote.ready(&mut control, seven, now), None, "once");
    // Never where it is blocked, turned off, or was not written.
    let mut blocked = Remote::default();
    blocked.initialized(Some(true));
    assert_eq!(blocked.ready(&mut control, seven, now), None);
    let mut ended = Remote::unblocked();
    ended.initialized(Some(true));
    ended.end(Some(&mut control), now);
    assert_eq!(ended.ready(&mut control, seven, now), None);
    remote.not_written(&mut control);
    assert!(matches!(remote.phase(), Phase::Dropped(_)) && remote.marked());
    assert_eq!(remote.ready(&mut control, seven, now), None);
}

#[test]
fn where_it_cannot_be_asked_it_is_dropped_and_still_marked() {
    let now = Instant::now();
    let dropped = |remote: &Remote| match remote.phase() {
        Phase::Dropped(why) => why.clone(),
        other => panic!("{other:?}"),
    };
    for available in [None, Some(false)] {
        let mut remote = Remote::unblocked();
        remote.initialized(available);
        assert_eq!(remote.ready(&mut on(now), seven, now), None);
        assert!(dropped(&remote).contains("not available"));
        assert!(remote.marked());
    }
    let mut tier_one = Control::started(now);
    tier_one.answered(&refused(INITIALIZE));
    let mut remote = Remote::unblocked();
    assert_eq!(remote.ready(&mut tier_one, seven, now), None);
    assert!(dropped(&remote).contains("takes no requests"));
    let mut remote = Remote::unblocked();
    remote.initialized(Some(true));
    let none = || Err(std::io::ErrorKind::Other.into());
    assert_eq!(remote.ready(&mut on(now), none, now), None);
    assert!(dropped(&remote).contains("random"));
}

#[test]
fn it_comes_back_only_to_the_session_it_was() {
    let now = Instant::now();
    let answered = |back: Option<RemoteId>, gave: Option<Enabled>| {
        let mut remote = Remote::coming_back(back);
        remote.initialized(Some(true));
        let asked = remote.ready(&mut on(now), seven, now).unwrap();
        let off = remote.answered(asked.id(), None, gave);
        (off, remote)
    };
    let (off, remote) = answered(Some(id("cse_a")), Some(enabled("cse_a")));
    assert!(!off);
    assert_eq!(remote.carried(), Some(&id("cse_a")));
    assert!(answered(Some(id("cse_a")), Some(enabled("cse_b"))).0);
    assert!(answered(Some(id("cse_a")), None).0, "no id");
    let (off, remote) = answered(None, Some(enabled("cse_b")));
    assert!(!off, "the person's own");
    assert!(remote.carried().is_some());
    let (off, remote) = answered(None, None);
    assert!(!off);
    assert_eq!(remote.carried(), None, "no id to come back to");
    // A refused request says so, and is no other session.
    let mut remote = Remote::coming_back(Some(id("cse_a")));
    remote.initialized(Some(true));
    let asked = remote.ready(&mut on(now), seven, now).unwrap();
    assert!(!remote.answered(asked.id(), Some("no"), None));
    assert_eq!(remote.carried(), None);
}

#[test]
fn turned_off_nothing_is_carried_over_a_restart() {
    let now = Instant::now();
    let mut remote = Remote::unblocked();
    let mut control = on(now);
    remote.initialized(Some(true));
    let asked = remote.ready(&mut control, seven, now).unwrap();
    remote.answered(asked.id(), None, Some(enabled("cse_a")));
    assert!(remote.carried().is_some());
    remote.event(&mut bridge("failed", ""));
    assert!(remote.carried().is_some(), "dropped, the person has it on");
    remote.end(Some(&mut control), now);
    assert_eq!(remote.carried(), None);
    remote.event(&mut bridge("connected", ""));
    assert_eq!(remote.carried(), None);
    assert_eq!(Remote::default().carried(), None);
}

#[test]
fn a_start_blocked_after_all_is_off_and_keeps_the_canary() {
    let mut remote = Remote::unblocked();
    assert!(!remote.blocked(), "nothing said it may be connected");
    assert!(!remote.marked());
    let mut remote = Remote::unblocked();
    remote.initialized(Some(true));
    assert!(remote.blocked(), "available, though it is blocked");
    assert!(!remote.marked() && remote.watched());
    let mut remote = Remote::unblocked();
    remote.event(&mut bridge("connected", ""));
    assert!(remote.blocked(), "connected, though it is blocked");
    assert!(!remote.marked() && remote.watched());
    assert_eq!(remote.mark(), None);
}

#[test]
fn the_mark_and_the_rows_say_where_it_is() {
    let now = Instant::now();
    assert_eq!(Remote::default().mark(), None);
    let mut remote = Remote::unblocked();
    assert_eq!(remote.mark(), Some("⇄ connecting"));
    assert_eq!(remote.take_row(), None);
    remote.event(&mut bridge("connected", ""));
    assert_eq!(remote.mark(), Some("⇄ claude.ai"));
    assert!(
        remote
            .take_row()
            .unwrap()
            .starts_with("Remote Control is on")
    );
    assert_eq!(remote.take_row(), None, "once");
    remote.event(&mut bridge("failed", "a\u{1b}[31m\nb\u{202e}"));
    assert_eq!(remote.mark(), Some("⇄ dropped"));
    let row = remote.take_row().unwrap();
    assert!(row.contains("may connect again"), "{row}");
    assert!(
        !row.contains(['\n', '\u{1b}', '\u{202e}']),
        "escaped: {row}"
    );
    // No line floods the rows, and none says it is off.
    let mut rows = 2;
    for n in 0..100 {
        remote.event(&mut bridge(["connected", "failed"][n % 2], &n.to_string()));
        if let Some(row) = remote.take_row() {
            assert!(!row.contains("is off"));
            rows += 1;
        }
    }
    assert_eq!(rows, MAX_ROWS);
    remote.end(None, now);
    assert_eq!(remote.mark(), Some("⇄ going off"));
    remote.event(&mut bridge("connected", ""));
    assert_eq!(remote.mark(), Some("⇄ going off"), "until it ended");
}
