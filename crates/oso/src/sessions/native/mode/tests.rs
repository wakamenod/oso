//! The permission mode's rules (ADR 0165, part 3b): what is shown and carried, the cycle, and a
//! switch from its request to its fallback, a resume in the new mode, or refused.
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::time::Instant;

use super::{ANSWER_WAIT, ModeState, Process, Refusal, Then};
use crate::channel::protocol::mode::PermissionMode::{self, AcceptEdits, Auto, Default, Plan};
use crate::sessions::native::control::tests::{ok, on};
use crate::sessions::native::control::{Answered, Control};
use crate::sessions::native::wire::control::{self, INITIALIZE, Request};

const IDLE: Process = Process {
    idle: true,
    spoke: true,
    resumable: true,
};
const BUSY: Process = Process {
    idle: false,
    ..IDLE
};

fn in_mode(name: &str) -> ModeState {
    let mut modes = ModeState::default();
    assert!(modes.said(name));
    modes
}

/// A control layer whose `initialize` was refused: tier 1.
fn tier_one(now: Instant) -> Control {
    let mut control = Control::started(now);
    let answered = control.answered(&crate::sessions::native::control::tests::refused(
        INITIALIZE,
    ));
    assert!(matches!(answered, Answered::Initialize(Some(_))));
    control
}

/// The id and mode a switch begun on tier 2 asks with.
fn asked(modes: &mut ModeState, control: &mut Control, now: Instant) -> (u64, PermissionMode) {
    match modes.begin(Some(control), IDLE, now) {
        Ok(Then::Request(id, mode)) => (id, mode),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_cycle_is_the_terminals_and_a_mode_oso_never_sends_goes_to_default() {
    let now = Instant::now();
    for (from, to) in [
        ("default", AcceptEdits),
        ("acceptEdits", Plan),
        ("plan", Auto),
        ("auto", Default),
        ("bypassPermissions", Default),
        ("dontAsk", Default),
    ] {
        let mut modes = in_mode(from);
        let mut control = on(now);
        assert_eq!(asked(&mut modes, &mut control, now).1, to, "from {from}");
    }
    let mut control = on(now);
    assert_eq!(
        ModeState::default().begin(Some(&mut control), IDLE, now),
        Err(Refusal::Unknown)
    );
}

#[test]
fn the_mode_shown_is_the_clis_and_odd_text_is_not_shown() {
    let mut modes = in_mode("plan");
    assert_eq!(modes.shown(), Some("plan"));
    assert!(!modes.said("plan"), "the same");
    assert!(!modes.said(""), "says nothing");
    assert!(modes.said("bypassPermissions"));
    assert!(!modes.said("bypassPermissions"), "the same");
    assert_eq!(modes.shown(), Some("bypassPermissions"));
    assert!(modes.said("plan\u{202e}"));
    assert_eq!(modes.shown(), None, "not readable as itself");
    assert!(modes.said("acceptEdits"));
    assert_eq!(modes.shown(), Some("acceptEdits"));
}

#[test]
fn a_new_process_carries_a_mode_that_asks_or_one_the_person_chose_and_none_else() {
    assert_eq!(in_mode("plan").for_start(), Some(Plan));
    assert_eq!(in_mode("default").for_start(), Some(Default));
    // The stream is not authenticated: a line a tool wrote can say a mode that asks less.
    assert_eq!(in_mode("auto").for_start(), None, "not the person's");
    assert_eq!(in_mode("acceptEdits").for_start(), None, "not the person's");
    assert_eq!(in_mode("bypassPermissions").for_start(), None);
    assert_eq!(in_mode("dontAsk").for_start(), None);
    assert_eq!(ModeState::default().for_start(), None, "the settings' mode");

    // Chosen by the person's switch, it is carried while the CLI says it.
    let now = Instant::now();
    let (mut control, mut modes) = (on(now), in_mode("default"));
    let (id, _) = asked(&mut modes, &mut control, now);
    assert_eq!(modes.answered(&mut control, id, None, IDLE), Vec::new());
    assert!(modes.said("acceptEdits"));
    assert_eq!(modes.for_start(), Some(AcceptEdits));
    assert!(!modes.said("acceptEdits"), "the new process says it again");
    assert!(modes.said("auto"), "a line no switch asked for");
    assert_eq!(modes.for_start(), None);
}

#[test]
fn a_mode_oso_gave_is_carried_and_switched_from_until_the_process_says_one() {
    let now = Instant::now();
    // Tier 2: a second switch before the CLI's word goes on from the first.
    let (mut control, mut modes) = (on(now), in_mode("default"));
    let (id, _) = asked(&mut modes, &mut control, now);
    assert_eq!(modes.answered(&mut control, id, None, IDLE), Vec::new());
    assert_eq!(asked(&mut modes, &mut control, now).1, Plan);

    // Tier 1: a step down from `auto` stays down, though the process ends before it speaks.
    let mut control = tier_one(now);
    let mut modes = in_auto();
    assert_eq!(modes.carried(), Some(Auto), "the person chose it");
    assert_eq!(
        modes.begin(Some(&mut control), IDLE, now),
        Ok(Then::Respawn(Default))
    );
    assert_eq!(modes.for_start(), Some(Default));
    assert!(!modes.ended(false), "the process ended before it spoke");
    assert_eq!(modes.carried(), Some(Default), "not the `auto` it left");
    assert!(modes.said("default"), "the next process says it");
    assert_eq!(modes.shown(), Some("default"));
    assert_eq!(modes.carried(), Some(Default));
}

/// A session the person's switch put in `auto` on tier 2, as the CLI then says.
fn in_auto() -> ModeState {
    let now = Instant::now();
    let mut control = on(now);
    let mut modes = in_mode("plan");
    let (id, mode) = asked(&mut modes, &mut control, now);
    assert_eq!(mode, Auto);
    assert_eq!(modes.answered(&mut control, id, None, IDLE), Vec::new());
    assert!(modes.said("auto"));
    modes
}

#[test]
fn a_switch_whose_process_ends_without_resuming_is_over() {
    let now = Instant::now();
    let mut control = tier_one(now);
    let mut modes = in_mode("plan");
    assert_eq!(
        modes.begin(Some(&mut control), IDLE, now),
        Ok(Then::Respawn(Auto))
    );
    assert!(!modes.ended(true), "resumes");
    assert!(modes.respawns());
    assert!(modes.ended(false), "came to nothing");
    assert!(!modes.respawns());
    assert_eq!(modes.carried(), Some(Plan), "the mode it was in");
    assert_eq!(modes.begin(None, IDLE, now), Err(Refusal::NotRunning));

    let (mut control, mut modes) = (on(now), in_mode("default"));
    asked(&mut modes, &mut control, now);
    assert!(!modes.ended(false));
    assert_eq!(modes.deadline(), None, "a request that waited is over");
    let held = Process {
        resumable: false,
        ..IDLE
    };
    let mut control = tier_one(now);
    assert_eq!(
        modes.begin(Some(&mut control), held, now),
        Err(Refusal::Held)
    );
}

#[test]
fn on_tier_two_a_switch_is_a_request_that_changes_nothing_shown() {
    let now = Instant::now();
    let mut control = on(now);
    let mut modes = in_mode("default");
    let (id, mode) = asked(&mut modes, &mut control, now);
    assert_eq!(mode, AcceptEdits);
    let line: serde_json::Value =
        serde_json::from_str(&control::request(id, Request::SetPermissionMode(mode))).unwrap();
    assert_eq!(line["request"]["subtype"], "set_permission_mode");
    assert_eq!(line["request"]["mode"], "acceptEdits");
    assert_eq!(
        modes.begin(Some(&mut control), IDLE, now),
        Err(Refusal::Already)
    );
    assert_eq!(modes.deadline(), Some(now + ANSWER_WAIT));
    assert_eq!(
        modes.answered(&mut control, id + 1, None, IDLE),
        Vec::new(),
        "not its answer"
    );
    assert_eq!(modes.answered(&mut control, id, None, IDLE), Vec::new());
    assert_eq!(modes.deadline(), None);
    assert_eq!(modes.shown(), Some("default"), "until the CLI says so");
    assert!(!modes.respawns());
    assert!(modes.said("acceptEdits"));
    assert_eq!(asked(&mut modes, &mut control, now).1, Plan);
}

#[test]
fn a_request_unanswered_or_refused_falls_back_to_a_resume_while_idle() {
    let now = Instant::now();
    let resumed = |why: &str| vec![Then::Log(why.to_owned()), Then::Respawn(AcceptEdits)];

    let (mut control, mut modes) = (on(now), in_mode("default"));
    let (id, _) = asked(&mut modes, &mut control, now);
    assert_eq!(modes.due(&mut control, IDLE, now), Vec::new(), "not yet");
    assert_eq!(
        modes.due(&mut control, IDLE, now + ANSWER_WAIT),
        resumed("set_permission_mode was not answered")
    );
    assert!(modes.respawns());
    assert_eq!(control.answered(&ok(id)), Answered::Other, "forgotten");
    assert_eq!(modes.deadline(), None);
    assert_eq!(modes.shown(), Some("default"));
    assert_eq!(modes.for_start(), Some(AcceptEdits), "the resume's mode");
    assert!(!modes.respawns());
    assert_eq!(
        modes.for_start(),
        Some(AcceptEdits),
        "until the process says one"
    );
    assert!(modes.said("acceptEdits"));
    assert_eq!(modes.for_start(), Some(AcceptEdits), "the person chose it");

    let (mut control, mut modes) = (on(now), in_mode("default"));
    let (id, _) = asked(&mut modes, &mut control, now);
    assert_eq!(
        modes.answered(&mut control, id, Some("invalid_mode"), IDLE),
        resumed("set_permission_mode was refused: invalid_mode")
    );

    let (mut control, mut modes) = (on(now), in_mode("default"));
    asked(&mut modes, &mut control, now);
    assert_eq!(
        modes.not_written(&mut control, IDLE),
        resumed("the set_permission_mode request was not written")
    );
}

#[test]
fn during_a_turn_a_fallback_is_refused_until_idle() {
    let now = Instant::now();
    let (mut control, mut modes) = (on(now), in_mode("default"));
    asked(&mut modes, &mut control, now);
    assert_eq!(
        modes.due(&mut control, BUSY, now + ANSWER_WAIT),
        vec![
            Then::Log("set_permission_mode was not answered".to_owned()),
            Then::Refused(Refusal::Busy)
        ]
    );
    assert!(!modes.respawns());
    assert_eq!(modes.deadline(), None, "fired once");
    assert_eq!(modes.due(&mut control, BUSY, now + ANSWER_WAIT), Vec::new());
    let (id, _) = asked(&mut modes, &mut control, now);
    assert!(id > 0, "a new switch may begin");
}

#[test]
fn on_tier_one_a_switch_resumes_an_idle_session_and_is_refused_else() {
    let now = Instant::now();
    let mut control = tier_one(now);
    let mut modes = in_mode("plan");
    assert_eq!(
        modes.begin(Some(&mut control), BUSY, now),
        Err(Refusal::Busy)
    );
    let unspoken = Process {
        spoke: false,
        ..IDLE
    };
    assert_eq!(
        modes.begin(Some(&mut control), unspoken, now),
        Err(Refusal::Unspoken)
    );
    assert_eq!(
        modes.begin(Some(&mut control), IDLE, now),
        Ok(Then::Respawn(Auto))
    );
    assert!(modes.respawns());
    assert_eq!(
        modes.begin(Some(&mut control), IDLE, now),
        Err(Refusal::Already)
    );
    assert_eq!(modes.for_start(), Some(Auto));
}

#[test]
fn with_no_process_or_one_starting_nothing_changes() {
    let now = Instant::now();
    let mut modes = in_mode("default");
    assert_eq!(modes.begin(None, IDLE, now), Err(Refusal::NotRunning));
    let mut starting = Control::started(now);
    assert_eq!(
        modes.begin(Some(&mut starting), IDLE, now),
        Err(Refusal::Starting)
    );
    assert!(!modes.respawns());
}

#[test]
fn a_new_process_ends_a_request_that_waited() {
    let now = Instant::now();
    let (mut control, mut modes) = (on(now), in_mode("plan"));
    asked(&mut modes, &mut control, now);
    assert_eq!(modes.for_start(), Some(Plan));
    assert_eq!(modes.deadline(), None);
    let mut control = on(now);
    assert_eq!(asked(&mut modes, &mut control, now).1, Auto);
}

#[test]
fn initialize_says_the_mode_the_session_is_in() {
    let answer = |line: &str| match crate::sessions::native::wire::parse(line.as_bytes()) {
        crate::sessions::native::wire::Event::Answer(answer) => answer.mode,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        answer(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"oso-0","response":{"current_permission_mode":"plan"}}}"#
        ),
        Some("plan".to_owned())
    );
    assert_eq!(
        answer(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"oso-0","response":{}}}"#
        ),
        None
    );
}
