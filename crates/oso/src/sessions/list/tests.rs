//! `*Sessions*`: its groups and their order, pins, nesting, folds, the narrow and wide layouts,
//! the line map, and its cost.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; these tests drive the sessions' own
//! parts, with no window.

use std::time::{Duration, Instant};

use super::{Entry, Group, Key, Line, List, WIDE_FROM, age, cells, cut};
use crate::sessions::state::{Reason, State};

const ASK: State = State::NeedsInput(Reason::Permission);

fn entry(key: u64, name: &str, state: State, since: Instant) -> Entry {
    Entry {
        key: Key::Hosted(key),
        name: name.to_owned(),
        space: "oso".to_owned(),
        branch: Some("main".to_owned()),
        state,
        since,
        recap: format!("{name} recap"),
        pinned: false,
        archived: false,
        parent: None,
    }
}

fn headers(lines: &[Line]) -> Vec<Group> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Header(group) => Some(group.clone()),
            Line::Session(_) => None,
        })
        .collect()
}

/// The sessions in the order shown, once each.
fn order(lines: &[Line]) -> Vec<Key> {
    let mut keys: Vec<Key> = Vec::new();
    for line in lines {
        if let Line::Session(key) = line
            && keys.last() != Some(key)
        {
            keys.push(*key);
        }
    }
    keys
}

#[test]
fn groups_follow_the_taxonomy_and_pins_and_waits_order_a_group() {
    let t = Instant::now();
    let mut entries = vec![
        entry(1, "idle", State::Idle, t),
        entry(2, "late-ask", ASK, t + Duration::from_secs(5)),
        entry(3, "work", State::Working, t),
        entry(4, "early-ask", ASK, t),
        entry(5, "pinned-ask", ASK, t + Duration::from_secs(9)),
        entry(6, "failed", State::Failed, t),
    ];
    entries[4].pinned = true;
    let list = List::default();
    let shown = list.render(&entries, 32, t + Duration::from_secs(60));
    assert_eq!(
        headers(&shown.lines),
        [
            Group::State(0),
            Group::State(1),
            Group::State(2),
            Group::State(5)
        ]
    );
    assert_eq!(
        order(&shown.lines),
        [5, 4, 2, 6, 3, 1].map(Key::Hosted),
        "pinned first, then the longest waiting"
    );
    assert!(shown.text.starts_with("▾ Needs input 3\n⚠ pinned-ask"));
}

#[test]
fn by_space_groups_by_name_and_archived_come_last_folded() {
    let t = Instant::now();
    let mut entries = vec![
        entry(1, "a", State::Idle, t),
        entry(2, "b", ASK, t),
        entry(3, "old", State::Stopped, t),
    ];
    entries[0].space = "web".to_owned();
    entries[2].archived = true;
    let mut list = List::default();
    list.regroup();
    let shown = list.render(&entries, 80, t);
    assert_eq!(
        headers(&shown.lines),
        [
            Group::Space("oso".to_owned()),
            Group::Space("web".to_owned()),
            Group::Archived
        ]
    );
    assert_eq!(
        order(&shown.lines),
        [2, 1].map(Key::Hosted),
        "archived is folded"
    );
    assert!(shown.text.ends_with("▸ Archived 1\n"));
    list.fold(&Line::Header(Group::Archived));
    let opened = list.render(&entries, 80, t);
    assert_eq!(order(&opened.lines), [2, 1, 3].map(Key::Hosted));
    list.fold(&Line::Header(Group::Space("oso".to_owned())));
    assert_eq!(
        order(&list.render(&entries, 80, t).lines),
        [1, 3].map(Key::Hosted)
    );
}

#[test]
fn sessions_nest_under_their_parent_which_counts_those_that_need_input() {
    let t = Instant::now();
    let mut entries = vec![
        entry(1, "lead", State::Working, t),
        entry(2, "child-a", ASK, t),
        entry(3, "child-b", ASK, t),
        entry(4, "grandchild", State::Done, t),
    ];
    for (child, parent) in [(1, 1), (2, 1), (3, 3)] {
        entries[child].parent = Some(Key::Hosted(parent));
    }
    let mut list = List::default();
    list.regroup();
    let shown = list.render(&entries, 32, t);
    assert_eq!(order(&shown.lines), [1, 2, 3, 4].map(Key::Hosted));
    let text: Vec<&str> = shown.text.lines().collect();
    assert_eq!(text[3], "  2 nested need input · lead re…", "{text:?}");
    assert!(text[4].starts_with("  ⚠ child-a"), "indented: {text:?}");
    list.fold(&Line::Session(Key::Hosted(1)));
    assert_eq!(order(&list.render(&entries, 32, t).lines), [Key::Hosted(1)]);
    // By state the children wait in their own group, at the top level.
    list.regroup();
    let by_state = list.render(&entries, 32, t);
    assert_eq!(order(&by_state.lines), [2, 3, 1, 4].map(Key::Hosted));
}

#[test]
fn a_cycle_of_parents_ends() {
    let t = Instant::now();
    let mut entries = vec![entry(1, "a", ASK, t), entry(2, "b", ASK, t)];
    entries[0].parent = Some(Key::Hosted(2));
    entries[1].parent = Some(Key::Hosted(1));
    let shown = List::default().render(&entries, 32, t);
    assert_eq!(
        order(&shown.lines),
        [1, 2].map(Key::Hosted),
        "both at the top level"
    );
}

#[test]
fn a_narrow_window_takes_three_lines_a_session_each_within_its_width() {
    let t = Instant::now();
    let mut long = entry(1, &"n".repeat(40), ASK, t);
    long.recap = "Do you want to proceed with this rather long command?".to_owned();
    long.branch = Some("feature/界面".to_owned());
    let shown = List::default().render(&[long], 32, t + Duration::from_secs(720));
    let text: Vec<&str> = shown.text.lines().collect();
    assert_eq!(text.len(), 4);
    assert_eq!(text[1], format!("⚠ {}… 12m", "n".repeat(25)));
    assert_eq!(text[2], "  oso · feature/界面");
    assert_eq!(text[3], format!("  {}…", &"Do you want to proceed with t"));
    for line in &text {
        assert!(cells(line) <= 32, "{line:?}");
    }
    assert_eq!(shown.lines.len(), 4);
    assert_eq!(shown.lines[3], Line::Session(Key::Hosted(1)));
}

#[test]
fn a_wide_window_takes_one_line_a_session_in_columns() {
    let t = Instant::now();
    let mut external = entry(9, "other", State::Working, t);
    external.key = Key::External(4242);
    external.branch = None;
    let shown = List::default().render(&[external], WIDE_FROM, t);
    let text: Vec<&str> = shown.text.lines().collect();
    assert_eq!(text.len(), 2);
    assert_eq!(
        text[1],
        "▶ other                oso · external               0s    o…"
    );
    assert!(cells(text[1]) <= WIDE_FROM);
}

#[test]
fn ages_and_cuts() {
    let t = Instant::now();
    let after = |s| age(t, t + Duration::from_secs(s));
    assert_eq!(
        [0, 59, 60, 3599, 3600, 86_400].map(after),
        ["0s", "59s", "1m", "59m", "1h", "1d"]
    );
    assert_eq!(cut("abc", 3), "abc");
    assert_eq!(cut("abcd", 3), "ab…");
    assert_eq!(cut("界界界", 4), "界…");
    assert_eq!(cut("a\nb", 9), "a");
    assert_eq!(cut("abc", 0), "");
    assert_eq!(cells("a\u{202E}"), 1 + cells("\u{202E}"));
}

#[test]
fn two_hundred_fifty_six_sessions_render_within_the_budget() {
    let t = Instant::now();
    let entries: Vec<Entry> = (0..256)
        .map(|n| {
            let state = [ASK, State::Working, State::Done, State::Idle][n % 4];
            let mut entry = entry(n as u64, &format!("session-{n}"), state, t);
            entry.parent = (n % 5 == 1).then(|| Key::Hosted(n as u64 - 1));
            entry.recap = "Do you want to make this edit to src/main.rs?".to_owned();
            entry
        })
        .collect();
    let list = List::default();
    let mut best = Duration::MAX;
    for _ in 0..10 {
        let start = Instant::now();
        let shown = list.render(&entries, 32, t);
        best = best.min(start.elapsed());
        assert!(shown.lines.len() > 256 * 3);
    }
    // A debug build is several times slower; the budget is a release build's (ADR 0121).
    assert!(cfg!(debug_assertions) || best.as_micros() < 500, "{best:?}");
}
