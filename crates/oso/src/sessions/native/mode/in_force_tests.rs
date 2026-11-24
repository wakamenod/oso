//! The mode in force, which the footer and the modeline say: the running process's own word, and
//! whether a change of it is under way.
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::time::Instant;

use super::ModeState;
use super::tests::{IDLE, asked, in_mode};
use crate::channel::protocol::mode::PermissionMode::AcceptEdits;
use crate::sessions::native::control::tests::on;

/// What is in force is what the process that runs now said, and nothing it did not say yet.
#[test]
fn the_mode_in_force_is_the_one_this_process_said() {
    let mut modes = in_mode("plan");
    assert_eq!(modes.in_force(), Some("plan"));
    assert!(!modes.changing());
    // Its process ends, and another starts in the same mode: not said yet, then said again.
    modes.ended(true);
    assert_eq!(modes.in_force(), None, "no process is in it");
    modes.for_start();
    assert_eq!((modes.in_force(), modes.shown()), (None, Some("plan")));
    assert!(modes.said("plan", false), "it reads differently now");
    assert!(!modes.said("plan", false));
    assert_eq!(modes.in_force(), Some("plan"));
    // One that cannot be shown leaves none in force.
    assert!(modes.said("plan\u{202e}", false));
    assert_eq!(modes.in_force(), None);
}

/// From the key until the process says a mode, the one in force stays, and is being changed.
#[test]
fn a_switch_is_a_change_under_way_until_the_process_says_so() {
    let now = Instant::now();
    let mut modes = in_mode("default");
    let mut control = on(now);
    let (id, mode) = asked(&mut modes, &mut control, now);
    assert_eq!(mode, AcceptEdits);
    assert!(modes.changing());
    assert_eq!(modes.in_force(), Some("default"));
    assert_eq!(modes.answered(&mut control, id, None, IDLE), []);
    assert!(modes.changing(), "answered, and not said");
    assert_eq!(modes.in_force(), Some("default"));
    assert!(modes.said("acceptEdits", false));
    assert!(!modes.changing());
    assert_eq!(modes.in_force(), Some("acceptEdits"));
    // To the mode it is in already: a success and no word of it (measured).
    modes.chose(AcceptEdits);
    assert!(!modes.changing());
}

/// A mode said that cannot be shown is told from none said, whatever came before it.
#[test]
fn a_mode_that_cannot_be_shown_is_told_from_none_said() {
    let odd = "auto\u{202e}";
    let mut modes = ModeState::default();
    assert!(!modes.unshown(), "none said");
    assert!(modes.said(odd, false), "it reads differently");
    assert!(modes.unshown() && modes.in_force().is_none());
    assert!(!modes.said(odd, false));
    modes.for_start();
    assert!(!modes.unshown(), "another process said none");
    let mut known = in_mode("plan");
    assert!(!known.unshown());
    assert!(known.said(odd, true));
    assert!(known.unshown() && known.strays_now());
    assert_eq!(known.strayed(), None, "no word of it to show");
}
