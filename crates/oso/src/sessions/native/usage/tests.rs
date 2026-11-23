//! `get_usage`'s rules (ADR 0165, part 3a): asked once a process on tier 2, its answer taken
//! only while it waits, and forgotten once its wait ends.
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::time::Instant;

use super::{ANSWER_WAIT, GetUsage};
use crate::sessions::native::control::tests::{ok, on, refused};
use crate::sessions::native::control::{Answered, Control};
use crate::sessions::native::wire::control::{self, INITIALIZE, Request};

#[test]
fn it_is_asked_once_and_only_on_tier_two() {
    let now = Instant::now();
    let mut get = GetUsage::default();
    let mut tier_one = Control::started(now);
    assert_eq!(get.ask(&mut tier_one, now), None, "initialize not answered");
    tier_one.answered(&refused(INITIALIZE));
    assert_eq!(get.ask(&mut tier_one, now), None, "refused: tier 1");
    let mut control = on(now);
    let id = get.ask(&mut control, now).unwrap();
    assert_eq!(get.deadline(), Some(now + ANSWER_WAIT));
    assert_eq!(get.ask(&mut control, now), None, "once a process");
    assert_eq!(get.answered(id + 1, None), None, "another request's");
    assert_eq!(get.answered(id, None), Some(Ok(())));
    assert_eq!(get.answered(id, None), None, "taken once");
    assert_eq!(get.deadline(), None);
    assert_eq!(get.ask(&mut control, now), None);
}

#[test]
fn it_comes_through_the_control_layer_as_a_get_usage_answer() {
    let now = Instant::now();
    let mut get = GetUsage::default();
    let mut control = on(now);
    let id = get.ask(&mut control, now).unwrap();
    assert_eq!(
        control.answered(&ok(id)),
        Answered::Request {
            request: Request::GetUsage,
            id,
            error: None
        }
    );
}

#[test]
fn a_refused_answer_is_logged_and_gives_nothing() {
    let now = Instant::now();
    let mut get = GetUsage::default();
    let mut control = on(now);
    let id = get.ask(&mut control, now).unwrap();
    assert_eq!(
        get.answered(id, Some("no")),
        Some(Err("get_usage was refused: no".to_owned()))
    );
    assert_eq!(get.deadline(), None);
}

#[test]
fn unanswered_it_is_forgotten_and_a_late_answer_is_none() {
    let now = Instant::now();
    let mut get = GetUsage::default();
    let mut control = on(now);
    let id = get.ask(&mut control, now).unwrap();
    assert_eq!(get.due(&mut control, now), None);
    let late = now + ANSWER_WAIT;
    assert_eq!(
        get.due(&mut control, late).as_deref(),
        Some("get_usage was not answered")
    );
    assert_eq!(get.due(&mut control, late), None, "said once");
    assert_eq!(get.answered(id, None), None);
    assert_eq!(
        control.answered(&ok(id)),
        Answered::Other,
        "the control layer forgot it too"
    );
}

#[test]
fn not_written_it_is_not_asked_again() {
    let now = Instant::now();
    let mut get = GetUsage::default();
    let mut control = on(now);
    let id = get.ask(&mut control, now).unwrap();
    get.not_written(&mut control);
    assert_eq!(get.ask(&mut control, now), None);
    assert_eq!(control.answered(&ok(id)), Answered::Other);
}

#[test]
fn the_request_line_is_get_usage() {
    let line = control::request(3, Request::GetUsage);
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["request_id"], "oso-3");
    assert_eq!(value["request"]["subtype"], "get_usage");
}
