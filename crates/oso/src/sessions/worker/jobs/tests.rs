//! The worker's jobs, with made-up instants: coalescing, the debounce's own wake-ups, rules
//! swapped for every session, and reading a saved screen.
//!
//! Must not depend on: `app` or `dispatch`; these tests drive the sessions' own
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
        let found = core.handle(Job::ExplainFile(path.clone()), Instant::now());
        let [Out::Unexplained(why)] = found.as_slice() else {
            panic!("{found:?}");
        };
        assert!(why.contains(says), "{why}");
        // The echo area, which the agent channel reads, never holds what the person typed.
        let typed = path.file_name().unwrap().to_string_lossy();
        assert!(!why.contains(typed.as_ref()), "{why}");
    }
    fs::remove_dir_all(&dir).unwrap();
}

/// A core whose rules always run out of time, with session 1 past its grace period at the
/// instant returned.
fn too_slow(start: Instant) -> (Core, Arc<RuleSet>, Instant) {
    let parsed = parse(RULES, Source::Bundled);
    let rules = Arc::new(RuleSet::merged(parsed, Default::default()));
    let mut core = Core::with_budget(Arc::clone(&rules), Duration::ZERO);
    core.handle(Job::Start(1), start);
    (core, rules, start + GRACE)
}

#[test]
fn rules_too_slow_twice_in_a_row_hold_and_are_said_once_for_each_load() {
    let (mut core, rules, at) = too_slow(Instant::now());
    core.handle(screen("esc to interrupt"), at);
    assert!(core.tick(at).is_empty(), "one late look says nothing");
    assert_eq!(
        core.due(),
        Some(at + ms(200)),
        "the same screen again, after twice the wait"
    );
    assert_eq!(core.tick(at + ms(200)), [Out::Slow("busy".to_owned())]);
    assert_eq!(core.due(), Some(at + ms(600)), "doubled again: 400 ms");
    assert!(
        core.tick(at + ms(600)).is_empty(),
        "said once, and the state holds"
    );
    core.handle(Job::Rules(rules), at + ms(700));
    assert!(core.tick(at + ms(700)).is_empty());
    assert_eq!(core.tick(at + ms(900)), [Out::Slow("busy".to_owned())]);
}

#[test]
fn a_late_look_at_a_still_screen_is_judged_again() {
    let (mut core, _, at) = too_slow(Instant::now());
    core.handle(screen("esc to interrupt"), at);
    assert!(core.tick(at).is_empty(), "late: the state holds");
    // The machine is less busy now; the screen has not changed.
    core.budget = Duration::from_secs(60);
    let due = core.due().expect("judged again");
    assert_eq!(states(&core.tick(due)), [State::Working]);
}

#[test]
fn a_look_that_found_what_the_last_did_is_not_judged_again() {
    let start = Instant::now();
    let (mut core, at) = working(start);
    core.handle(screen("esc to interrupt"), at + ms(10));
    assert_eq!(core.due(), None);
    core.handle(screen("esc to interrupt!"), at + ms(20));
    assert_eq!(core.due(), Some(at + EVALUATE_EVERY));
}

/// Judges session 1's screen at each due time from `at`, `looks` times, and returns what was
/// found and the last instant.
fn look_again(core: &mut Core, mut at: Instant, looks: usize) -> (Vec<Out>, Instant) {
    let mut found = core.tick(at);
    for _ in 1..looks {
        at = core.due().expect("judged again");
        found.extend(core.tick(at));
    }
    (found, at)
}

#[test]
fn a_local_rule_late_three_looks_in_a_row_is_dropped_until_the_rules_load() {
    let start = Instant::now();
    let local = parse(RULES, Source::Local);
    let rules = Arc::new(RuleSet::merged(Default::default(), local));
    let mut core = Core::with_budget(Arc::clone(&rules), Duration::ZERO);
    core.handle(Job::Start(1), start);
    let at = start + GRACE;
    core.handle(screen("esc to interrupt"), at);
    let (found, at) = look_again(&mut core, at, 3);
    assert_eq!(
        found,
        [
            Out::Slow("busy".to_owned()),
            Out::Dropped("busy".to_owned())
        ]
    );
    // With it gone nothing is late, and no rule matches.
    core.handle(screen("esc to interrupt."), at);
    let due = core.due().unwrap();
    let found = core.tick(due);
    assert!(
        found
            .iter()
            .all(|out| matches!(out, Out::Judged(_, Some(false)))),
        "idle, as it was, and judged in time at last: {found:?}"
    );
    assert_eq!(core.due(), None, "judged in time");
    core.handle(Job::Rules(rules), due);
    let (found, _) = look_again(&mut core, due, 3);
    assert!(
        found.contains(&Out::Dropped("busy".to_owned())),
        "used again, and dropped again"
    );
}

#[test]
fn a_bundled_rule_is_never_dropped() {
    let (mut core, _, at) = too_slow(Instant::now());
    core.handle(screen("esc to interrupt"), at);
    let (found, _) = look_again(&mut core, at, 6);
    assert_eq!(found, [Out::Slow("busy".to_owned())]);
}

#[test]
fn a_recap_is_told_only_when_it_changes() {
    let rules = "version = 1\n[[rule]]\nname = \"busy\"\nstate = \"working\"\n\
                 region = \"bottom(1)\"\ncontains = [\"esc to interrupt\"]\n\
                 recap = '^(\\S+) · '\n";
    let parsed = parse(rules, Source::Bundled);
    assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
    let mut core = Core::new(Arc::new(RuleSet::merged(parsed, Default::default())));
    let recaps = |found: Vec<Out>| -> Vec<String> {
        found
            .into_iter()
            .filter_map(|out| match out {
                Out::Recap(1, recap) => Some(recap),
                _ => None,
            })
            .collect()
    };
    let mut at = Instant::now();
    core.handle(Job::Start(1), at);
    for (text, told) in [
        ("Reading · esc to interrupt", &["Reading"][..]),
        ("Reading · 2s · esc to interrupt", &[]),
        ("Writing · esc to interrupt", &["Writing"]),
    ] {
        at += GRACE;
        core.handle(screen(text), at);
        assert_eq!(recaps(core.tick(at)), told, "{text}");
    }
}

#[test]
fn the_first_look_in_time_and_each_change_of_hold_are_told() {
    let rules = "version = 1\n[[rule]]\nname = \"view\"\nstate = \"hold\"\n\
                 region = \"bottom(1)\"\ncontains = [\"showing detailed transcript\"]\n";
    let parsed = parse(rules, Source::Bundled);
    assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
    let mut core = Core::new(Arc::new(RuleSet::merged(parsed, Default::default())));
    let judged = |found: Vec<Out>| -> Vec<bool> {
        found
            .into_iter()
            .filter_map(|out| match out {
                Out::Judged(1, Some(held)) => Some(held),
                _ => None,
            })
            .collect()
    };
    let mut at = Instant::now();
    core.handle(Job::Start(1), at);
    for (text, told) in [
        ("❯ ", &[false][..]),
        ("❯ hello", &[]),
        ("showing detailed transcript", &[true]),
        ("showing detailed transcript ·", &[]),
        ("❯ ", &[false]),
    ] {
        at += GRACE;
        core.handle(screen(text), at);
        assert_eq!(judged(core.tick(at)), told, "{text}");
    }
}

#[test]
fn a_session_is_told_judged_only_by_looks_in_time_and_told_when_one_runs_late() {
    let judged = |found: &[Out]| -> Vec<Option<bool>> {
        found
            .iter()
            .filter_map(|out| match out {
                Out::Judged(1, held) => Some(*held),
                _ => None,
            })
            .collect()
    };
    let start = Instant::now();
    let rules = Arc::new(RuleSet::merged(
        Default::default(),
        parse(RULES, Source::Local),
    ));
    let mut core = Core::with_budget(Arc::clone(&rules), Duration::ZERO);
    core.handle(Job::Start(1), start);
    let at = start + GRACE;
    // Late from the start, until the slow local rule is dropped: never judged.
    core.handle(screen("esc to interrupt"), at);
    let (found, at) = look_again(&mut core, at, 3);
    assert_eq!(judged(&found), []);
    // With it gone, a look in time.
    core.handle(screen("esc to interrupt."), at);
    let due = core.due().unwrap();
    assert_eq!(judged(&core.tick(due)), [Some(false)]);
    // The rules load again with it, and the next look is late: no longer judged.
    core.handle(Job::Rules(rules), due);
    let (found, _) = look_again(&mut core, due, 1);
    assert_eq!(judged(&found), [None]);
}
