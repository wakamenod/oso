//! Tests of what `session-adopt` offers: which conversations, in which order, named and
//! annotated how, and the names kept from the registry.
//!
//! Must not depend on: `app` or `dispatch`, a registry or transcripts on disk.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::{Found, MAX_SEEN, Running, Seen, choices, running};
use crate::sessions::identity::SessionId;
use crate::sessions::registry::Entry;
use crate::sessions::transcripts::Conversation;

fn id(n: u8) -> SessionId {
    SessionId::from_random([n; 16])
}

fn short(n: u8) -> String {
    id(n).as_str()[..8].to_owned()
}

fn entry(pid: u32, n: u8, status: &str) -> Entry {
    Entry {
        pid,
        session_id: Some(id(n)),
        cwd: Some(PathBuf::from("/p/oso")),
        proc_start: None,
        started_at: None,
        name: Some(format!("live-{n}")),
        kind: None,
        status: Some(status.to_owned()),
    }
}

fn stopped(n: u8, title: Option<&str>, ago: u64, now: SystemTime) -> Conversation {
    Conversation {
        id: id(n),
        changed: now - Duration::from_secs(ago),
        directory: PathBuf::from("/p/oso"),
        title: title.map(str::to_owned),
    }
}

fn texts(found: &Found, seen: &Seen, now: SystemTime) -> Vec<(String, String)> {
    choices(found, seen, now)
        .candidates
        .iter()
        .map(|c| (c.text().to_owned(), c.annotation().to_owned()))
        .collect()
}

#[test]
fn running_ones_are_those_that_may_run_and_are_not_left_out() {
    let entries = vec![
        entry(30, 1, "busy"),
        entry(20, 2, "idle"),
        entry(10, 3, "idle"),
    ];
    let left_out = HashSet::from([id(2)]);
    let started = |pid: u32| match pid {
        30 => Err(io::Error::other("unknown")),
        _ => Ok(None),
    };
    // Pid 30's start cannot be told, so it counts as running; 10 runs no more; 20 is left out.
    let found = running(entries, &left_out, started);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, 30);
    assert!(found[0].busy);
}

#[test]
fn live_ones_come_first_with_their_process_then_stopped_ones_by_age() {
    let now = SystemTime::now();
    let found = Found {
        running: vec![Running {
            pid: 42,
            id: id(1),
            name: Some("live".into()),
            directory: Some("/p/oso".into()),
            busy: false,
        }],
        stopped: vec![stopped(2, Some("fix tests"), 90, now)],
        cache: Default::default(),
    };
    let rows = texts(&found, &Seen::default(), now);
    assert_eq!(rows[0].0, "live");
    assert_eq!(
        rows[0].1,
        "runs as process 42: stop it there first · /p/oso"
    );
    assert_eq!(rows[1].0, "fix tests");
    assert_eq!(rows[1].1, "1m ago · /p/oso");
}

#[test]
fn a_shared_name_or_none_gets_the_ids_first_digits() {
    let now = SystemTime::now();
    let found = Found {
        running: Vec::new(),
        stopped: vec![
            stopped(1, Some("same"), 10, now),
            stopped(2, Some("same"), 20, now),
            stopped(3, None, 30, now),
        ],
        cache: Default::default(),
    };
    let made = choices(&found, &Seen::default(), now);
    let rows: Vec<&str> = made.candidates.iter().map(|c| c.text()).collect();
    assert_eq!(
        rows,
        [
            format!("same {}", short(1)),
            format!("same {}", short(2)),
            format!("oso {}", short(3))
        ]
    );
    assert_eq!(made.picks[&format!("oso {}", short(3))].id, id(3));
}

#[test]
fn a_name_seen_while_it_ran_is_kept_after_it_stopped() {
    let now = SystemTime::now();
    let mut seen = Seen::default();
    seen.note(&id(1), Some("from-registry"), true);
    let found = Found {
        running: Vec::new(),
        stopped: vec![stopped(1, Some("transcript title"), 5, now)],
        cache: Default::default(),
    };
    let made = choices(&found, &seen, now);
    let pick = &made.picks["from-registry"];
    assert!(pick.busy, "it was busy when last seen");
    assert_eq!(pick.running, None);
}

#[test]
fn what_others_wrote_is_shown_on_one_row() {
    let now = SystemTime::now();
    let mut conversation = stopped(1, Some("a\nb\u{202e}c"), 5, now);
    conversation.directory = PathBuf::from(format!("/p/{}\n", "d".repeat(300)));
    let found = Found {
        running: Vec::new(),
        stopped: vec![conversation],
        cache: Default::default(),
    };
    for (text, annotation) in texts(&found, &Seen::default(), now) {
        for shown in [text, annotation] {
            assert!(!shown.contains('\n'), "{shown}");
            assert!(!shown.contains('\u{202e}'), "{shown}");
            assert!(shown.len() < 260, "{}", shown.len());
        }
    }
}

#[test]
fn at_most_so_many_names_are_kept() {
    let mut seen = Seen::default();
    for n in 0..=MAX_SEEN {
        let id = SessionId::from_random([
            (n % 256) as u8,
            (n / 256) as u8,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1,
        ]);
        seen.note(&id, Some("n"), false);
    }
    let first = SessionId::from_random([0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    assert!(seen.get(&first).is_none(), "the oldest went");
    assert_eq!(seen.by_id.len(), MAX_SEEN);
}
