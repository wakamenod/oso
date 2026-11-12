//! The worker's jobs, with made-up instants: coalescing, the debounce's own wake-ups, rules
//! swapped for every session, and reading a saved screen.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no terminal.

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{Core, MAX_EXPLAIN_BYTES};
use crate::sessions::rules::{RuleSet, Source, parse};
use crate::sessions::screen::Evidence;
use crate::sessions::state::{Exit, GRACE, State};
use crate::sessions::worker::{EVALUATE_EVERY, Job, Out};

const RULES: &str = r#"
version = 1
[[rule]]
name = "busy"
state = "working"
region = "bottom(1)"
contains = ["esc to interrupt"]
"#;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn core() -> Core {
    let parsed = parse(RULES, Source::Bundled);
    Core::new(Arc::new(RuleSet::merged(parsed, Default::default())))
}

fn screen(text: &str) -> Job {
    Job::Screen(1, Evidence::from_text(text))
}

fn states(found: &[Out]) -> Vec<State> {
    found
        .iter()
        .filter_map(|out| match out {
            Out::Changed(1, change) => Some(change.state),
            _ => None,
        })
        .collect()
}

/// A core with session 1 started at `start`, past its grace period and working.
fn working(start: Instant) -> (Core, Instant) {
    let mut core = core();
    core.handle(Job::Start(1), start);
    let at = start + GRACE;
    core.handle(screen("esc to interrupt"), at);
    assert_eq!(states(&core.tick(at)), [State::Working]);
    (core, at)
}

#[test]
fn nothing_waits_with_no_screen_and_a_screen_is_judged_at_once() {
    let start = Instant::now();
    let mut core = core();
    assert_eq!(core.due(), None);
    core.handle(Job::Start(1), start);
    assert_eq!(core.due(), None, "no timer for a session with nothing new");
    let (core, at) = working(start);
    assert_eq!(core.due(), None, "nothing waits once judged");
    let _ = at;
}

#[test]
fn screens_faster_than_ten_a_second_are_coalesced() {
    let start = Instant::now();
    let (mut core, at) = working(start);
    core.handle(screen("one"), at + ms(10));
    assert!(core.tick(at + ms(10)).is_empty(), "too soon after the last");
    assert_eq!(core.due(), Some(at + EVALUATE_EVERY));
    core.handle(screen("esc to interrupt"), at + ms(50));
    assert!(
        core.tick(at + EVALUATE_EVERY).is_empty(),
        "the newest kept: working"
    );
    assert_eq!(core.due(), None);
}

#[test]
fn the_debounce_wakes_the_worker_until_the_turn_ends() {
    let start = Instant::now();
    let (mut core, at) = working(start);
    core.handle(screen("done"), at + ms(100));
    assert!(core.tick(at + ms(100)).is_empty());
    let mut now = at + ms(100);
    let mut ended = Vec::new();
    for _ in 0..10 {
        let Some(due) = core.due() else { break };
        now = due;
        ended.extend(core.tick(now));
    }
    assert_eq!(states(&ended), [State::Done]);
    assert_eq!(now, at + ms(300), "3 looks 100 ms apart");
    assert_eq!(core.due(), None);
}

#[test]
fn new_rules_judge_every_session_again() {
    let start = Instant::now();
    let (mut core, at) = working(start);
    let local = parse(
        "version = 1\n[[rule]]\nname = \"busy\"\ndisable = true",
        Source::Local,
    );
    let swapped = RuleSet::merged(parse(RULES, Source::Bundled), local);
    core.handle(Job::Rules(Arc::new(swapped)), at + ms(500));
    let mut found = core.tick(at + ms(500));
    while let Some(due) = core.due() {
        found.extend(core.tick(due));
    }
    assert_eq!(states(&found), [State::Done]);
}

#[test]
fn exits_looks_and_reviews_reach_the_tracker_and_forget_drops_it() {
    let start = Instant::now();
    let (mut core, at) = working(start);
    assert!(
        core.handle(Job::Seen(1), at).is_empty(),
        "working is not done"
    );
    assert_eq!(
        states(&core.handle(Job::Exited(1, Exit::Failed), at + ms(1))),
        [State::Failed]
    );
    core.handle(Job::Forget(1), at + ms(2));
    assert!(core.handle(Job::Changes(1, true), at + ms(3)).is_empty());
    assert!(core.handle(Job::ReviewOpened(1), at + ms(3)).is_empty());
    assert!(core.handle(screen("x"), at + ms(4)).is_empty());
    assert_eq!(core.due(), None);
}

#[test]
fn a_saved_screen_is_explained_and_odd_files_are_refused() {
    let dir = std::env::temp_dir().join(format!("oso-explain-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let saved = dir.join("screen.txt");
    fs::write(&saved, b"out \xff\n  esc to interrupt\n").unwrap();
    let mut core = core();
    let found = core.handle(Job::ExplainFile(saved.clone()), Instant::now());
    let [Out::Explained(text)] = found.as_slice() else {
        panic!("{found:?}");
    };
    assert!(text.contains("Rule: busy (bundled)"), "{text}");
    let big = dir.join("big.txt");
    fs::write(&big, vec![b'x'; MAX_EXPLAIN_BYTES as usize + 1]).unwrap();
    for (path, says) in [
        (big, "larger than 256 KiB"),
        (dir.clone(), "not a regular file"),
        (dir.join("none"), "Cannot explain"),
    ] {
        let found = core.handle(Job::ExplainFile(path), Instant::now());
        let [Out::Unexplained(why)] = found.as_slice() else {
            panic!("{found:?}");
        };
        assert!(why.contains(says), "{why}");
    }
    fs::remove_dir_all(&dir).unwrap();
}
