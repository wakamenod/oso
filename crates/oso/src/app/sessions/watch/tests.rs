//! The tests of what the app makes of sessions' states without a window: the modeline's
//! marks, and a time remembered in ms made an instant.
//!
//! Must not depend on: a window, the worker's thread or the clock's exact value.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{marks, since_ms};
use crate::sessions::list::{Entry, Key};
use crate::sessions::state::{Reason, State};

fn entry(key: u64, state: State, archived: bool) -> Entry {
    Entry {
        key: Key::Hosted(key),
        name: format!("s{key}"),
        space: String::new(),
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
fn the_marks_count_what_waits_on_the_person_archived_ones_aside() {
    let entries = [
        entry(1, State::NeedsInput(Reason::Permission), false),
        entry(2, State::NeedsInput(Reason::Question), false),
        entry(3, State::NeedsInput(Reason::Plan), true),
        entry(4, State::Done, false),
        entry(5, State::ReadyForReview, false),
        entry(6, State::Working, false),
        entry(7, State::Failed, false),
        entry(8, State::Done, true),
    ];
    let shown = marks(&entries, false);
    assert_eq!((shown.needs_input, shown.done, shown.dark), (2, 2, false));
    assert!(marks(&entries, true).dark);
    assert_eq!(marks(&[], false), Default::default());
}

#[test]
fn a_change_remembered_in_ms_is_as_far_back_as_it_was() {
    let now = Instant::now();
    let ms = |back: Duration| {
        let at = SystemTime::now() - back;
        u64::try_from(at.duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap()
    };
    let since = since_ms(ms(Duration::from_secs(90)), now);
    let back = now.duration_since(since);
    assert!(
        back >= Duration::from_secs(89) && back <= Duration::from_secs(91),
        "{back:?}"
    );
    // One in the future, as a clock set back gives, is now.
    assert_eq!(since_ms(u64::MAX, now), now);
}
