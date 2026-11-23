//! The control layer's rules (ADR 0165, step 2c): when a request goes, which request an answer
//! is to, and why tier 2 is off, said once; and the control lines on the wire.
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::time::Instant;

use super::{Answered, Control, INIT_WAIT, RECEIPT};
use crate::sessions::native::wire::control::{self, Answer, INITIALIZE, Request};
use crate::sessions::native::wire::{self, Event, MAX_NAME};

pub(in crate::sessions::native) fn ok(id: u64) -> Answer {
    Answer {
        id: Some(id),
        ..Answer::default()
    }
}

pub(in crate::sessions::native) fn refused(id: u64) -> Answer {
    Answer {
        id: Some(id),
        error: Some("no".to_owned()),
        ..Answer::default()
    }
}

/// A control layer whose `initialize` was answered and whose turn lists [`RECEIPT`].
pub(in crate::sessions::native) fn on(now: Instant) -> Control {
    let mut control = Control::started(now);
    assert_eq!(
        control.answered(&ok(INITIALIZE)),
        Answered::Initialize(None)
    );
    assert_eq!(control.system_init(&[RECEIPT.to_owned()]), None);
    assert!(control.has(Request::Interrupt));
    control
}

#[test]
fn an_interrupt_request_needs_the_initialize_answer_and_no_system_init_without_the_receipt() {
    let now = Instant::now();
    let mut control = Control::started(now);
    assert_eq!(control.system_init(&[RECEIPT.to_owned()]), None);
    assert!(
        !control.has(Request::Interrupt),
        "initialize not answered yet"
    );
    control.answered(&ok(INITIALIZE));
    assert!(control.has(Request::Interrupt));
    // Before the first `system/init`, the answer is enough.
    let mut first = Control::started(now);
    first.answered(&ok(INITIALIZE));
    assert!(first.has(Request::Interrupt), "no system/init yet");
    assert!(
        !control.has(Request::Initialize),
        "written once, at the start"
    );
    assert_eq!(
        control.system_init(&["something_else_v1".to_owned()]),
        Some(format!("system/init does not list {RECEIPT}"))
    );
    assert!(
        !control.has(Request::Interrupt),
        "a CLI that stops listing it"
    );
    assert_eq!(control.system_init(&[]), None, "said once");
}

#[test]
fn an_initialize_refused_or_unanswered_is_said_once_and_a_late_success_still_counts() {
    let now = Instant::now();
    let mut control = Control::started(now);
    assert_eq!(control.deadline(), Some(now + INIT_WAIT));
    assert_eq!(control.due(now), None);
    let late = now + INIT_WAIT;
    assert_eq!(
        control.due(late),
        Some("initialize was not answered".to_owned())
    );
    assert_eq!(control.deadline(), None);
    assert_eq!(control.due(late), None, "said once");
    assert_eq!(
        control.answered(&refused(INITIALIZE)),
        Answered::Initialize(None),
        "an error after the wait was said as no answer"
    );
    control.answered(&ok(INITIALIZE));
    control.system_init(&[RECEIPT.to_owned()]);
    assert!(control.has(Request::Interrupt));

    let mut control = Control::started(now);
    assert_eq!(
        control.answered(&refused(INITIALIZE)),
        Answered::Initialize(Some("initialize was refused: no".to_owned()))
    );
    assert_eq!(control.due(late), None);
}

#[test]
fn an_answer_goes_to_the_request_that_waits_by_its_id_once() {
    let now = Instant::now();
    let mut control = on(now);
    let first = control.ask(Request::Interrupt);
    let second = control.ask(Request::Interrupt);
    assert_ne!(first, second);
    assert_ne!(first, INITIALIZE);
    assert_eq!(control.answered(&ok(second + 1)), Answered::Other);
    assert_eq!(
        control.answered(&ok(INITIALIZE)),
        Answered::Other,
        "initialize's answer after one that succeeded"
    );
    assert_eq!(
        control.answered(&Answer {
            id: None,
            error: None,
            ..Answer::default()
        }),
        Answered::Other
    );
    assert_eq!(
        control.answered(&refused(first)),
        Answered::Request {
            request: Request::Interrupt,
            id: first,
            error: Some("no".to_owned()),
        }
    );
    assert_eq!(control.answered(&ok(first)), Answered::Other, "once");
    control.forget(second);
    assert_eq!(control.answered(&ok(second)), Answered::Other);
}

#[test]
fn the_requests_and_their_answers_go_through_the_wire() {
    let line = control::request(3, Request::Interrupt);
    assert!(line.ends_with('\n'));
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["type"], "control_request");
    assert_eq!(value["request_id"], "oso-3");
    assert_eq!(value["request"]["subtype"], "interrupt");
    let line = control::request(INITIALIZE, Request::Initialize);
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["request"]["subtype"], "initialize");

    let answer = |line: &str| match wire::parse(line.as_bytes()) {
        Event::Answer(answer) => answer,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        answer(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"oso-3","response":{"still_queued":[]}}}"#
        ),
        ok(3)
    );
    assert_eq!(
        answer(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"oso-0","error":"Unsupported control request subtype: x"}}"#
        ),
        Answer {
            id: Some(0),
            error: Some("Unsupported control request subtype: x".to_owned()),
            ..Answer::default()
        }
    );
    assert_eq!(
        answer(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"req_1"}}"#
        ),
        Answer {
            id: None,
            error: Some("no reason given".to_owned()),
            ..Answer::default()
        },
        "another's id is no answer of oso's"
    );
    assert_eq!(
        wire::parse(br#"{"type":"control_response"}"#),
        Event::Other("control_response".to_owned())
    );
    // An error the CLI writes is cut, and logged escaped.
    let long = format!("\u{1b}[31m\u{202e}{}", "e".repeat(10_000));
    let line = serde_json::json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": "oso-0", "error": long},
    })
    .to_string();
    let error = answer(&line).error.unwrap();
    assert!(error.len() <= MAX_NAME);
    let mut control = Control::started(Instant::now());
    let Answered::Initialize(Some(said)) = control.answered(&Answer {
        id: Some(INITIALIZE),
        error: Some(error),
        ..Answer::default()
    }) else {
        panic!("a log line");
    };
    let logged = format!("{said:?}");
    assert!(
        !logged.contains('\u{1b}') && !logged.contains('\u{202e}'),
        "{logged}"
    );
}
