//! The tracker's transitions, with instants made up for each step.
//!
//! Must not depend on: `app` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no terminal.

use std::time::{Duration, Instant};

use super::{CONFIRM_AT_MOST, CONFIRM_EVERY, Change, Exit, GRACE, Reason, State, Target, Tracker};

const IDLE: Target = Target::Idle { visible: false };
const SIGN: Target = Target::Idle { visible: true };
const PERMISSION: Target = Target::NeedsInput(Reason::Permission);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A tracker past its grace period, working since `at`.
fn working(start: Instant) -> (Tracker, Instant) {
    let mut tracker = Tracker::new(start);
    let at = start + GRACE;
    tracker.verdict(Target::Working, at).unwrap();
    (tracker, at)
}

fn state(change: Option<Change>) -> Option<State> {
    change.map(|change| change.state)
}

#[test]
fn a_session_starts_idle_and_only_a_dialog_moves_it_in_its_grace_period() {
    let start = Instant::now();
    let mut tracker = Tracker::new(start);
    assert_eq!(tracker.state(), State::Idle);
    assert_eq!(tracker.due(), None, "no timer for a start");
    assert_eq!(tracker.verdict(Target::Working, start + ms(500)), None);
    assert_eq!(
        tracker.due(),
        Some(start + GRACE),
        "judged again when the grace period ends"
    );
    let change = tracker.verdict(PERMISSION, start + ms(900)).unwrap();
    assert_eq!(change.state, State::NeedsInput(Reason::Permission));
    assert!(!change.turn_ended, "a start is no turn");
    assert_eq!(tracker.due(), None);
    assert_eq!(
        tracker.verdict(SIGN, start + ms(1000)),
        None,
        "still in grace"
    );
    assert_eq!(
        state(tracker.verdict(SIGN, start + GRACE)),
        Some(State::Idle),
        "a dialog answered is not done"
    );
}

#[test]
fn a_visible_idle_sign_ends_a_turn_at_once() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    let change = tracker.verdict(SIGN, at + ms(10)).unwrap();
    assert_eq!(change.state, State::Done);
    assert!(change.turn_ended);
    assert_eq!(change.since, at + ms(10));
    assert_eq!(tracker.since(), at + ms(10));
}

#[test]
fn idle_without_a_sign_needs_three_verdicts_a_tenth_of_a_second_apart() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    assert_eq!(tracker.verdict(IDLE, at), None);
    assert_eq!(tracker.due(), Some(at + CONFIRM_EVERY));
    assert_eq!(
        tracker.verdict(IDLE, at + ms(50)),
        None,
        "too soon to count"
    );
    assert_eq!(tracker.confirming(), Some(1));
    assert_eq!(tracker.verdict(IDLE, at + ms(100)), None);
    assert_eq!(tracker.confirming(), Some(2));
    let change = tracker.verdict(IDLE, at + ms(200)).unwrap();
    assert_eq!(change.state, State::Done);
    assert!(change.turn_ended);
    assert_eq!(tracker.due(), None);
}

#[test]
fn work_seen_again_cancels_the_confirmation() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    tracker.verdict(IDLE, at);
    tracker.verdict(IDLE, at + ms(100));
    assert_eq!(tracker.verdict(Target::Working, at + ms(150)), None);
    assert_eq!(tracker.confirming(), None);
    assert_eq!(
        tracker.verdict(IDLE, at + ms(200)),
        None,
        "counts from one again"
    );
    assert_eq!(tracker.confirming(), Some(1));
}

#[test]
fn a_slow_confirmation_ends_by_the_cap() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    tracker.verdict(IDLE, at);
    assert_eq!(tracker.verdict(IDLE, at + ms(650)), None);
    assert_eq!(
        state(tracker.verdict(IDLE, at + CONFIRM_AT_MOST)),
        Some(State::Done)
    );
}

#[test]
fn a_dialog_during_a_turn_ends_it_at_once_and_a_hold_keeps_the_state() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    assert_eq!(tracker.verdict(Target::Hold, at + ms(10)), None);
    assert_eq!(tracker.state(), State::Working);
    let change = tracker.verdict(PERMISSION, at + ms(20)).unwrap();
    assert!(change.turn_ended, "ADR 0121: working to needs input");
    assert_eq!(
        state(tracker.verdict(Target::Working, at + ms(30))),
        Some(State::Working)
    );
    tracker.verdict(IDLE, at + ms(40));
    assert_eq!(tracker.verdict(Target::Hold, at + ms(50)), None);
    assert_eq!(
        tracker.confirming(),
        None,
        "a full-screen view stops the count"
    );
}

#[test]
fn done_becomes_idle_once_seen_and_idle_again_is_no_change() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    tracker.verdict(SIGN, at);
    assert_eq!(tracker.verdict(SIGN, at + ms(500)), None);
    assert_eq!(tracker.verdict(IDLE, at + ms(600)), None);
    assert_eq!(state(tracker.seen(at + ms(700))), Some(State::Idle));
    assert_eq!(tracker.seen(at + ms(800)), None);
}

#[test]
fn changes_after_a_turn_wait_for_a_review() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    tracker.verdict(SIGN, at);
    assert_eq!(tracker.changes(false, at), None);
    assert_eq!(
        state(tracker.changes(true, at + ms(5))),
        Some(State::ReadyForReview)
    );
    assert_eq!(
        tracker.seen(at + ms(10)),
        None,
        "seeing it is not reviewing it"
    );
    assert_eq!(state(tracker.review_opened(at + ms(20))), Some(State::Idle));
    tracker.verdict(Target::Working, at + ms(30));
    assert_eq!(
        tracker.changes(true, at + ms(40)),
        None,
        "a late answer while the next turn runs"
    );
}

#[test]
fn an_exit_ends_everything_after_it() {
    let start = Instant::now();
    let (mut tracker, at) = working(start);
    tracker.verdict(IDLE, at);
    assert_eq!(
        state(tracker.exited(Exit::Failed, at + ms(10))),
        Some(State::Failed)
    );
    assert_eq!(tracker.due(), None);
    assert_eq!(tracker.verdict(Target::Working, at + ms(20)), None);
    let mut stopped = Tracker::new(start);
    assert_eq!(
        state(stopped.exited(Exit::Stopped, start)),
        Some(State::Stopped)
    );
}

#[test]
fn a_state_names_its_reason() {
    assert_eq!(
        State::NeedsInput(Reason::Elicitation).to_string(),
        "needs input (elicitation)"
    );
}
