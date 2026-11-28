//! Tests of motion by pages: form feeds at a line's start and elsewhere, counts, the buffer's
//! edges and what a step may read (ADR 0190).
//!
//! Must not depend on: `buffer`, `window` or `editor`.

use super::super::text::testing::{display, marked};
use super::super::{WalkBudget, cluster_start};
use super::forward_page;
use crate::motion::Edge;
use crate::selection::Selection;
use proptest::prelude::*;

/// Where one page at a time from `from` stops in `text`, in the way `n` goes, until the
/// buffer's edge.
fn stops(text: &str, from: usize, n: isize) -> Vec<usize> {
    let d = display(text);
    let mut selection = Selection::at(from);
    let mut stops = Vec::new();
    let mut budget = WalkBudget::new();
    while forward_page(&d, &mut selection, n, &mut budget).is_ok() {
        assert!(stops.len() < 64, "{stops:?}");
        stops.push(selection.cursor);
    }
    assert!(!budget.is_spent());
    stops
}

/// Checks that `text`, with a `|` at each page's start, has those stops forward from its start
/// and back from its end, and the buffer's ends.
fn check(text: &str) {
    let (text, mut pages) = marked(text);
    let mut ahead = pages.clone();
    ahead.push(text.len());
    assert_eq!(stops(&text, 0, 1), ahead, "forward in {text:?}");
    pages.insert(0, 0);
    pages.reverse();
    assert_eq!(stops(&text, text.len(), -1), pages, "back in {text:?}");
}

#[test]
fn a_form_feed_at_a_lines_start_ends_a_page() {
    check("a\n\u{c}|b\n\u{c}|c");
    check("one\n\u{c}|\ntwo\n\u{c}|\n");
}

#[test]
fn a_form_feed_elsewhere_does_not() {
    check("a\u{c}b \u{c}c");
}

#[test]
fn a_form_feed_at_the_start_is_passed_from_it() {
    // From the start, forward goes past the first page's delimiter to the next one's, as
    // Emacs's `forward-page` does at a line's start.
    let text = "\u{c}a\n\u{c}b";
    assert_eq!(stops(text, 0, 1), [4, text.len()]);
    assert_eq!(stops(text, text.len(), -1), [4, 1, 0]);
    assert_eq!(stops(text, 1, 1), [4, text.len()]);
}

#[test]
fn counts_go_on_and_the_edges_stop_them() {
    let text = "a\n\u{c}b\n\u{c}c\n\u{c}d";
    let d = display(text);
    let mut budget = WalkBudget::new();
    let mut selection = Selection::at(0);
    assert_eq!(forward_page(&d, &mut selection, 2, &mut budget), Ok(()));
    assert_eq!(selection.cursor, 6);
    assert_eq!(forward_page(&d, &mut selection, -1, &mut budget), Ok(()));
    assert_eq!(selection.cursor, 3);
    assert_eq!(
        forward_page(&d, &mut selection, 9, &mut budget),
        Err(Edge::End)
    );
    assert_eq!(selection.cursor, text.len());
    assert_eq!(
        forward_page(&d, &mut selection, -9, &mut budget),
        Err(Edge::Start)
    );
    assert_eq!(selection.cursor, 0);
    assert!(stops("", 0, 1).is_empty());
}

#[test]
fn a_step_past_its_budget_stays() {
    let text = "a\n".repeat(4096);
    let d = display(&text);
    for (from, n) in [(0, 1), (text.len(), -1)] {
        let mut selection = Selection::at(from);
        let mut budget = WalkBudget::with_limit(4096);
        let _ = forward_page(&d, &mut selection, n, &mut budget);
        assert!(budget.is_spent());
        assert_eq!(selection.cursor, from);
    }
}

proptest! {
    /// Every stop is where a cursor can be, and each step goes the way it is told.
    #[test]
    fn stops_are_cluster_starts_in_order(
        pieces in prop::collection::vec(
            prop::sample::select(vec!["a", "\n", "\u{c}", "\r\n", "\u{301}", "あ"]),
            0..40,
        ),
        from in 0usize..80,
    ) {
        let text: String = pieces.concat();
        let d = display(&text);
        let from = cluster_start(&d, text.floor_char_boundary(from.min(text.len())));
        for n in [1, -1] {
            let mut at = from;
            loop {
                let mut selection = Selection::at(at);
                let mut budget = WalkBudget::new();
                let edge = forward_page(&d, &mut selection, n, &mut budget);
                prop_assert!(!budget.is_spent());
                let to = selection.cursor;
                if edge.is_err() {
                    prop_assert_eq!(to, at);
                    break;
                }
                prop_assert!(if n > 0 { to > at } else { to < at }, "{} to {} in {:?}", at, to, text);
                prop_assert_eq!(cluster_start(&d, to), to, "{:?}", text);
                at = to;
            }
        }
    }
}
