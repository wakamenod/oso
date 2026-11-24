//! A session that may be connected to claude.ai in `*Spaces*` (ADR 0181): its sign before its name,
//! at any width, and no name, title or reason that looks like it.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data.

use std::time::Instant;

use oso_core::syntax::Highlight;

use super::tests::entry;
use super::{Key, List, cells, unmarked};
use crate::sessions::state::{Reason, State};

/// The row of `entry` by state, `width` wide, and what its sign is drawn as.
fn row(entry: super::Entry, width: usize, now: Instant) -> (String, Vec<Highlight>) {
    let shown = List::by_state().render(&[entry], width, now);
    let at = shown
        .lines
        .iter()
        .position(|line| *line == super::Line::Session(Key::Hosted(1)))
        .unwrap();
    let row = shown.text.lines().nth(at).unwrap().to_owned();
    let start: usize = shown.text.lines().take(at).map(|l| l.len() + 1).sum();
    let sign = row.find('⇄').map(|sign| start + sign);
    let faces = (shown.highlights.iter())
        .filter(|(range, _)| sign.is_some_and(|sign| range.contains(&sign)))
        .map(|(_, face)| *face)
        .collect();
    (row, faces)
}

#[test]
fn the_sign_stands_before_the_name_at_any_width() {
    let now = Instant::now();
    for state in [State::Idle, State::Working, State::Stopped, State::Done] {
        for width in 8..60 {
            let mut connected = entry(1, "a-session-with-a-long-name", state, now);
            connected.remote = true;
            let (row, faces) = row(connected, width, now);
            assert!(row.contains("⇄ a"), "{width}: {row:?}");
            assert!(cells(&row) <= width, "{width}: {row:?}");
            assert_eq!(faces, [Highlight::SessionAlert]);
        }
    }
    let (plain, _) = row(entry(1, "wa", State::Idle, now), 40, now);
    assert!(!plain.contains('⇄'));
}

#[test]
fn one_that_waits_shows_it_too() {
    let now = Instant::now();
    let mut asking = entry(1, "wa", State::NeedsInput(Reason::Permission), now);
    asking.remote = true;
    assert!(row(asking, 40, now).0.contains("⇄ wa"));
}

#[test]
fn no_name_title_or_reason_begins_with_the_sign() {
    let now = Instant::now();
    for text in ["⇄ wa", "  ⇄⇄ wa", "○ ⇄ wa"] {
        assert_eq!(unmarked(text), "wa", "{text:?}");
        let (row, _) = row(entry(1, text, State::Idle, now), 40, now);
        assert!(!row.contains('⇄'), "{row:?}");
    }
    let mut failed = entry(1, "wa", State::Failed, now);
    failed.recap = "⇄ wa".into();
    let shown = List::by_state().render(&[failed], 40, now);
    assert!(!shown.text.contains('⇄'), "{}", shown.text);
}
