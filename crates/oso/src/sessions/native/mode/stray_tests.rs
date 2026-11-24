//! A mode that no key here gave while Remote Control may be connected (ADR 0181): where the
//! person's switch goes from it, and the rows that say it.
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::time::Instant;

use super::{MAX_STRAY_ROWS, ModeState, Process, Then};
use crate::channel::protocol::mode::PermissionMode::{AcceptEdits, Auto, Default};
use crate::sessions::native::control::tests::on;
use crate::sessions::native::wire::{Event, parse};

const IDLE: Process = Process {
    idle: true,
    spoke: true,
    resumable: true,
};

fn status(mode: &str) -> Event {
    let line = format!(r#"{{"type":"system","subtype":"status","permissionMode":"{mode}"}}"#);
    parse(line.as_bytes())
}

/// Where the person's key goes.
fn key(modes: &mut ModeState) -> Then {
    let now = Instant::now();
    modes.begin(Some(&mut on(now)), IDLE, now).unwrap()
}

#[test]
fn the_persons_key_goes_to_default_first_from_a_mode_set_elsewhere() {
    for elsewhere in ["plan", "acceptEdits", "auto", "bypassPermissions"] {
        let mut modes = ModeState::default();
        modes.said("default", true);
        modes.event(&status(elsewhere), true);
        assert!(
            matches!(key(&mut modes), Then::Request(_, Default)),
            "{elsewhere}"
        );
        // Not watched, as before.
        let mut modes = ModeState::default();
        modes.said("default", false);
        modes.event(&status("plan"), false);
        assert!(matches!(key(&mut modes), Then::Request(_, Auto)));
    }
    // Once the person chose here, round again.
    let mut modes = ModeState::default();
    modes.said("default", true);
    modes.event(&status("plan"), true);
    modes.chose(Default);
    modes.event(&status("default"), true);
    assert!(matches!(key(&mut modes), Then::Request(_, AcceptEdits)));
    assert!(modes.take_stray_row().is_some_and(|row| row.contains(": ")));
}

#[test]
fn a_first_word_that_is_not_the_processs_own_is_one_set_elsewhere() {
    let mut modes = ModeState::default();
    modes.event(&status("plan"), true);
    assert!(matches!(key(&mut modes), Then::Request(_, Default)));
    // `initialize`'s and `system/init`'s are its own.
    let mut modes = ModeState::default();
    modes.said("plan", true);
    assert!(matches!(key(&mut modes), Then::Request(_, Auto)));
    assert_eq!(modes.take_stray_row(), None);
}

#[test]
fn a_row_says_it_once_a_change_and_no_line_floods_them() {
    let mut modes = ModeState::default();
    modes.said("default", true);
    assert_eq!(modes.take_stray_row(), None);
    modes.event(&status("auto"), true);
    assert_eq!(modes.strayed(), Some("auto"));
    let row = modes.take_stray_row().unwrap();
    assert!(row.contains("by no key here: auto."), "{row}");
    assert_eq!(modes.take_stray_row(), None);
    let rows = (0..100).filter(|n| {
        modes.event(&status(if n % 2 == 0 { "plan" } else { "auto" }), true);
        modes.take_stray_row().is_some()
    });
    assert_eq!(rows.count(), MAX_STRAY_ROWS - 1);
    // A new process may say more.
    modes.for_start();
    assert_eq!(modes.strayed(), None);
    modes.said("plan", true);
    modes.event(&status("auto"), true);
    assert!(modes.take_stray_row().is_some());
}

#[test]
fn the_persons_own_switch_gives_no_row() {
    let mut modes = ModeState::default();
    modes.said("default", true);
    modes.chose(AcceptEdits);
    modes.event(&status("acceptEdits"), true);
    assert_eq!(modes.strayed(), None);
    assert_eq!(modes.take_stray_row(), None);
}
