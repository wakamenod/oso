//! Tests of what `session-adopt` offers: which conversations, in which order, named and
//! annotated how, and the names kept from the registry.
//!
//! Must not depend on: `app` or `dispatch`, a registry or transcripts on disk.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{
    Found, MAX_REGISTRY_NAMES, Offered, RegistryNames, Running, choices, holding, running,
};
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

fn texts(found: &Found, seen: &RegistryNames, now: SystemTime) -> Vec<(String, String)> {
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
    let rows = texts(&found, &RegistryNames::default(), now);
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
    let made = choices(&found, &RegistryNames::default(), now);
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
    let mut seen = RegistryNames::default();
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
    for (text, annotation) in texts(&found, &RegistryNames::default(), now) {
        for shown in [text, annotation] {
            assert!(!shown.contains('\n'), "{shown}");
            assert!(!shown.contains('\u{202e}'), "{shown}");
            assert!(shown.len() < 260, "{}", shown.len());
        }
    }
}

#[test]
fn at_most_so_many_names_are_kept() {
    let mut seen = RegistryNames::default();
    for n in 0..=MAX_REGISTRY_NAMES {
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
    assert_eq!(seen.by_id.len(), MAX_REGISTRY_NAMES);
}

#[test]
fn no_two_candidates_share_a_text_whatever_the_names() {
    let now = SystemTime::now();
    // A name equal to what another's shared name becomes, one equal to a nameless one's text,
    // and the same conversation listed twice.
    let found = Found {
        running: Vec::new(),
        stopped: vec![
            stopped(1, Some("same"), 10, now),
            stopped(2, Some("same"), 20, now),
            stopped(3, Some(&format!("same {}", short(1))), 30, now),
            stopped(4, None, 40, now),
            stopped(5, Some(&format!("oso {}", short(4))), 50, now),
            stopped(1, Some("again"), 60, now),
        ],
        cache: Default::default(),
    };
    let made = choices(&found, &RegistryNames::default(), now);
    assert_eq!(made.picks.len(), made.candidates.len());
    assert_eq!(
        made.candidates.len(),
        5,
        "the conversation listed twice is offered once"
    );
    let texts: HashSet<&str> = made.candidates.iter().map(|c| c.text()).collect();
    assert_eq!(texts.len(), 5);
    // Each text stands for the conversation whose row shows it.
    for candidate in made.candidates.iter() {
        let pick = &made.picks[candidate.text()];
        let id = pick.id.as_str();
        let name = pick.name.as_deref().unwrap_or("oso");
        assert!(
            candidate.text() == name
                || candidate.text().ends_with(&id[..8])
                || candidate.text().ends_with(id),
            "{} for {id}",
            candidate.text()
        );
    }
    // A name is offered as it is if no other has it; the ones it looks like give way.
    assert_eq!(made.picks[&format!("same {}", short(1))].id, id(3));
    assert_eq!(made.picks[&format!("oso {}", short(4))].id, id(5));
    assert_eq!(made.picks[&format!("same {}", short(2))].id, id(2));
    assert_eq!(made.picks[&format!("same {}", id(1))].id, id(1));
    assert_eq!(made.picks[&format!("oso {}", id(4))].id, id(4));
}

#[test]
fn a_name_shows_no_dot_the_annotations_are_split_by() {
    let now = SystemTime::now();
    let found = Found {
        running: Vec::new(),
        stopped: vec![stopped(1, Some("fix · 2d ago · /safe"), 5, now)],
        cache: Default::default(),
    };
    let rows = texts(&found, &RegistryNames::default(), now);
    assert_eq!(rows[0].0, "fix  2d ago  /safe");
}

#[test]
fn a_directory_belongs_to_the_deepest_root_that_holds_it() {
    let roots = [
        (1, Path::new("/p")),
        (2, Path::new("/p/oso")),
        (3, Path::new("/p/osox")),
    ];
    assert_eq!(holding(Path::new("/p/oso/crates"), roots), Some(2));
    assert_eq!(holding(Path::new("/p/oso"), roots), Some(2));
    assert_eq!(holding(Path::new("/p/os"), roots), Some(1));
    assert_eq!(holding(Path::new("/q"), roots), None);
}

#[test]
fn an_answer_is_looked_up_in_the_list_its_prompt_showed() {
    let now = SystemTime::now();
    let list = |n: u8| {
        let found = Found {
            running: Vec::new(),
            stopped: vec![stopped(n, Some("fix"), 5, now)],
            cache: Default::default(),
        };
        choices(&found, &RegistryNames::default(), now)
    };
    let mut offered = Offered::default();
    let first = offered.list(list(1), false).expect("no prompt shows it");
    assert_eq!(first[0].text(), "fix");
    // A prompt shows the first list; a newer one, whose "fix" is another conversation, waits.
    assert!(offered.list(list(2), true).is_none());
    assert!(offered.waiting());
    assert_eq!(offered.pick("fix").unwrap().id, id(1));
    // Once no prompt shows it, the newer list is the editor's.
    assert!(offered.unshown().is_some());
    assert!(!offered.waiting());
    assert_eq!(offered.pick("fix").unwrap().id, id(2));
    assert!(offered.unshown().is_none());
}
