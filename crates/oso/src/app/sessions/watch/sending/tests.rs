//! Tests of the completions `session-mention` offers.
//!
//! Must not depend on: a window or a terminal; entries in, candidates out.

use std::time::Instant;

use oso_core::complete::Candidate;

use super::mention_candidates;
use crate::sessions::list::{Entry, Key};
use crate::sessions::state::State;

fn entry(key: Key, name: &str, state: State, archived: bool) -> Entry {
    Entry {
        key,
        name: name.to_owned(),
        space: "oso".to_owned(),
        branch: None,
        state,
        since: Instant::now(),
        recap: String::new(),
        pinned: false,
        archived,
        parent: None,
    }
}

#[test]
fn live_sessions_are_offered_by_name_with_where_they_run_and_ended_or_odd_names_are_not() {
    let entries = [
        entry(Key::Hosted(1), "api", State::Idle, false),
        entry(Key::External(42), "docs", State::Working, false),
        entry(Key::Hosted(2), "old", State::Stopped, false),
        entry(Key::Hosted(3), "broke", State::Failed, false),
        entry(Key::Hosted(4), "kept", State::Idle, true),
        entry(Key::External(43), "x\u{202e}y", State::Idle, false),
        entry(Key::External(44), "two words", State::Idle, false),
        entry(Key::External(45), "a\u{200b}b", State::Idle, false),
    ];
    let offered = mention_candidates(&entries);
    let names: Vec<&str> = offered.iter().map(Candidate::text).collect();
    assert_eq!(names, ["api", "docs"]);
    assert_eq!(offered[0].annotation(), "oso · in oso");
    assert_eq!(offered[1].annotation(), "oso · outside oso");
}
