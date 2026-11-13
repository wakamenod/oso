//! When a session's state is told about: the delay, what cancels it, the merge, the toggles
//! and the blink, with made-up instants.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no clock.

use std::time::{Duration, Instant};

use oso_platform::notify::Kind;

use super::{Attention, BLINK_PHASE, BLINKS, DELAY, MERGE, Seen, Toggles};
use crate::sessions::state::{Reason, State};

const ASK: State = State::NeedsInput(Reason::Permission);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Every session in `state`, unwatched.
fn still(state: State) -> impl Fn(u64) -> Option<Seen> {
    move |_| {
        Some(Seen {
            state,
            watched: false,
        })
    }
}

#[test]
fn a_state_that_holds_for_its_delay_is_told_once() {
    let t = Instant::now();
    let mut a = Attention::new(Toggles::default());
    a.changed(1, "fix-tests", ASK, false, false, t);
    assert_eq!(
        a.due(),
        Some(t + BLINK_PHASE),
        "the blink's first phase ends"
    );
    assert_eq!(a.tick(t + ms(999), still(ASK)), None);
    let notice = a.tick(t + DELAY, still(ASK)).expect("due");
    assert_eq!(notice.kind, Kind::NeedsInput);
    assert_eq!(notice.text, "fix-tests needs input");
    assert_eq!(a.tick(t + DELAY * 5, still(ASK)), None, "told once");
}

#[test]
fn a_state_that_moved_on_or_was_watched_is_not_told() {
    let t = Instant::now();
    let mut a = Attention::new(Toggles::default());
    a.changed(1, "a", ASK, false, false, t);
    a.changed(1, "a", State::Working, false, false, t + ms(500));
    assert_eq!(a.tick(t + DELAY, still(ASK)), None, "changed back in time");
    a.changed(2, "b", State::Done, true, true, t);
    assert_eq!(a.tick(t + DELAY, still(State::Done)), None, "watched");
    a.changed(3, "c", State::Done, false, false, t);
    let watched = |_| {
        Some(Seen {
            state: State::Done,
            watched: true,
        })
    };
    assert_eq!(a.tick(t + DELAY, watched), None, "looked at meanwhile");
    a.changed(4, "d", State::Done, false, false, t);
    assert_eq!(a.tick(t + DELAY, |_| None), None, "gone");
    a.changed(5, "e", State::Done, false, false, t);
    a.forget(5);
    assert_eq!(a.tick(t + DELAY, still(State::Done)), None, "forgotten");
}

#[test]
fn shown_in_an_unfocused_or_unselected_window_is_still_told() {
    let t = Instant::now();
    let mut a = Attention::new(Toggles::default());
    a.changed(1, "a", State::Done, true, false, t);
    let notice = a.tick(t + DELAY, still(State::Done)).unwrap();
    assert_eq!(notice.text, "a is done");
    assert!(!a.dark(t), "a shown session does not blink");
}

#[test]
fn states_due_within_two_seconds_are_told_as_one() {
    let t = Instant::now();
    let mut a = Attention::new(Toggles::default());
    // Shown in a window not selected: told, with no blink to wait for.
    for key in 1..=3 {
        a.changed(key, "x", ASK, true, false, t);
    }
    a.changed(4, "y", State::Failed, true, false, t);
    let mixed = |key| {
        Some(Seen {
            state: if key == 4 { State::Failed } else { ASK },
            watched: false,
        })
    };
    let notice = a.tick(t + DELAY, mixed).unwrap();
    assert_eq!(notice.kind, Kind::NeedsInput, "the most urgent");
    assert_eq!(notice.text, "3 sessions need input, 1 failed");
    // One more, due within the merge window: held until it ends.
    a.changed(5, "z", State::Done, false, false, t + ms(500));
    assert_eq!(a.tick(t + ms(1500), still(State::Done)), None);
    assert_eq!(a.due(), Some(t + DELAY + MERGE));
    a.changed(6, "w", State::Done, false, false, t + ms(1000));
    assert_eq!(a.tick(t + ms(2000), still(State::Done)), None);
    let notice = a.tick(t + DELAY + MERGE, still(State::Done)).unwrap();
    assert_eq!(
        (notice.kind, notice.text.as_str()),
        (Kind::Done, "2 sessions are done")
    );
    assert_eq!(a.due(), None, "nothing waits");
}

#[test]
fn toggles_choose_the_kinds_told() {
    let t = Instant::now();
    let mut a = Attention::new(Toggles::default());
    a.changed(1, "r", State::ReadyForReview, false, false, t);
    assert_eq!(a.tick(t + DELAY, still(State::ReadyForReview)), None);
    a.set_toggles(Toggles {
        needs_input: false,
        ready_for_review: true,
        ..Toggles::default()
    });
    a.changed(2, "q", ASK, false, false, t);
    a.changed(1, "r", State::ReadyForReview, false, false, t);
    let notice = a.tick(t + DELAY, still(State::ReadyForReview)).unwrap();
    assert_eq!(notice.text, "r is ready for review");
    for state in [State::Working, State::Idle, State::Stopped] {
        a.changed(3, "s", state, false, false, t + MERGE * 2);
        assert_eq!(a.tick(t + MERGE * 3, still(state)), None, "{state}");
    }
}

#[test]
fn an_unshown_session_that_needs_input_or_failed_blinks_three_times() {
    let t = Instant::now();
    let mut a = Attention::new(Toggles {
        needs_input: false,
        failed: false,
        ..Toggles::default()
    });
    a.changed(1, "a", State::Done, false, false, t);
    assert!(!a.dark(t), "done does not blink");
    a.changed(1, "a", State::Failed, false, false, t);
    assert!(a.dark(t + ms(1)) && !a.dark(t + BLINK_PHASE + ms(1)));
    let mut phases = Vec::new();
    while let Some(due) = a.due() {
        phases.push((due - t, a.dark(due)));
        a.tick(due, still(State::Failed));
    }
    let expected: Vec<(Duration, bool)> = (1..=2 * BLINKS)
        .map(|phase| (BLINK_PHASE * phase, phase % 2 == 0 && phase < 2 * BLINKS))
        .collect();
    assert_eq!(phases, expected, "dark, lit, three times, then nothing");
    assert!(!a.dark(t + ms(1)), "it stays lit");
}
