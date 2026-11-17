//! The refresher's rules on a fake clock: the debounce, one refresh at a time, a hidden
//! review waiting, and a refresh that cannot start not coming due again.
//!
//! Must not depend on: the app, a window or the review's thread; the clock is given.

use std::time::{Duration, Instant};

use super::{AT_MOST, Due, LOOK_EVERY, QUIET, Refresher};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn a_refresh_waits_for_the_marks_to_settle_or_at_most_a_second() {
    let start = Instant::now();
    let mut refresher = Refresher::default();
    refresher.mark(start, true);
    assert_eq!(refresher.deadline(), Some(start + QUIET));
    assert_eq!(refresher.due(start + ms(100), true), Due::Nothing);
    // Marks keep coming every 150 ms: the first one's second ends the wait.
    for n in 1..=6 {
        refresher.mark(start + ms(150 * n), true);
    }
    assert_eq!(refresher.deadline(), Some(start + AT_MOST));
    assert_eq!(refresher.due(start + AT_MOST, true), Due::Refresh);
    assert_eq!(refresher.deadline(), None, "taken as it came due");
}

#[test]
fn marks_during_a_refresh_make_one_more_once_it_ends_however_it_ends() {
    let start = Instant::now();
    let mut refresher = Refresher::default();
    refresher.started();
    refresher.mark(start, true);
    refresher.mark(start + ms(10), true);
    assert_eq!(refresher.deadline(), None, "none while it runs");
    // Failed or stopped ends it as being read does.
    refresher.ended(start + ms(50), true);
    assert_eq!(refresher.deadline(), Some(start + ms(50) + QUIET));
    refresher.started();
    refresher.ended(start + ms(500), true);
    assert_eq!(refresher.deadline(), None, "no mark came: nothing more");
}

#[test]
fn a_refresh_that_cannot_start_does_not_come_due_again() {
    let start = Instant::now();
    let mut refresher = Refresher::default();
    refresher.mark(start, true);
    assert_eq!(refresher.due(start + QUIET, true), Due::Refresh);
    // The host's refresh returned before asking: nothing is due after it.
    assert_eq!(refresher.deadline(), None);
    assert_eq!(refresher.due(start + QUIET + ms(1), true), Due::Nothing);
}

#[test]
fn a_hidden_review_looks_at_nothing_and_refreshes_once_shown() {
    let start = Instant::now();
    let mut refresher = Refresher::default();
    refresher.showing(start, false, false);
    refresher.mark(start, false);
    assert_eq!(refresher.deadline(), None);
    // Marks that settle once it is no longer shown wait too.
    let mut shown = Refresher::default();
    shown.mark(start, true);
    assert_eq!(shown.due(start + QUIET, false), Due::Nothing);
    assert_eq!(shown.deadline(), None);
    // Shown again: it looks at once, and the marks start a refresh.
    let later = start + ms(5_000);
    for refresher in [&mut refresher, &mut shown] {
        refresher.showing(later, true, true);
        assert_eq!(refresher.deadline(), Some(later));
        assert_eq!(refresher.due(later + QUIET, true), Due::Refresh);
    }
}

#[test]
fn a_shown_review_looks_each_second_but_not_while_it_refreshes() {
    let start = Instant::now();
    let mut refresher = Refresher::default();
    refresher.showing(start, true, true);
    assert_eq!(refresher.due(start, true), Due::Look);
    assert_eq!(refresher.deadline(), Some(start + LOOK_EVERY));
    refresher.started();
    assert_eq!(refresher.due(start + LOOK_EVERY, true), Due::Nothing);
    refresher.ended(start + LOOK_EVERY, true);
    // oso's window hidden: no more looks.
    refresher.showing(start + LOOK_EVERY, true, false);
    assert_eq!(refresher.deadline(), None);
    refresher.look_now(start + LOOK_EVERY);
    assert_eq!(refresher.deadline(), None, "nothing to look at once");
}
