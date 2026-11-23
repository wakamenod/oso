//! Remote Control's lines (ADR 0181): the recorded ones, the requests', the lines that are nearly
//! one, and the addresses and ids that are nearly claude.ai's.
//!
//! Must not depend on: the app or a process.

use super::{Asked, Came, ConnectionState, Enable, Enabled, Origin, Stage};
use crate::sessions::native::wire::control::{self, Request};
use crate::sessions::native::wire::{Event, MAX_NAME, parse};

const RECORDED: &str = include_str!("../../fixtures/2.1.330/remote.jsonl");
const UUID: &str = "00000000-0000-4000-8000-00000000000a";

fn line(text: &str) -> Event {
    parse(text.as_bytes())
}

/// Recorded line `at`.
fn recorded(at: usize) -> Event {
    line(RECORDED.lines().nth(at).unwrap())
}

#[test]
fn the_recorded_lines_read() {
    let Event::Answer(answer) = recorded(0) else {
        panic!()
    };
    assert_eq!(answer.remote_available, Some(true));
    assert_eq!(answer.remote, None);
    let Event::Answer(answer) = recorded(1) else {
        panic!()
    };
    assert_eq!((answer.id, &answer.error), (Some(1), &None));
    assert_eq!(answer.remote_available, None);
    let enabled = answer.remote.as_deref().unwrap();
    assert_eq!(
        enabled.url.as_ref().unwrap().as_str(),
        "https://claude.ai/code/session_01AbCdEfGhIjKlMnOpQrStUv"
    );
    assert_eq!(
        enabled.id.as_ref().unwrap().as_str(),
        "cse_01AbCdEfGhIjKlMnOpQrStUv"
    );
    let said = format!("{answer:?}");
    assert!(
        !said.contains("AbCd") && !said.contains("environment"),
        "neither is logged: {said}"
    );
    let state = |at| match recorded(at) {
        Event::Connection(bridge) => bridge,
        other => panic!("{other:?}"),
    };
    assert_eq!(state(2).state, ConnectionState::Ready);
    assert_eq!(state(4).state, ConnectionState::Connected);
    assert_eq!(state(14).state, ConnectionState::Failed);
    assert!(state(14).detail.starts_with("Remote Control is disabled"));
    for (at, stage) in [(6, Stage::Comes), (7, Stage::Comes), (12, Stage::Over)] {
        match recorded(at) {
            Event::Lifecycle(said) => assert_eq!((said.uuid.as_str(), said.stage), (UUID, stage)),
            other => panic!("{other:?}"),
        }
    }
    for (at, origin) in [(5, Origin::Unsaid), (8, Origin::Human)] {
        match recorded(at) {
            Event::User(user) => assert_eq!((user.origin, user.came), (origin, Came::AsSaid)),
            other => panic!("{other:?}"),
        }
    }
    for at in [15, 16] {
        match recorded(at) {
            Event::Answer(answer) => assert!(answer.error.is_some()),
            other => panic!("{other:?}"),
        }
    }
    // The CLI asks oso for nothing: a cancel of its question closes nothing.
    assert_eq!(
        recorded(9),
        Event::Other("control_cancel_request".to_owned())
    );
}

#[test]
fn the_requests_lines() {
    let body = |line: &str| {
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["type"], "control_request");
        assert_eq!(value["request_id"], "oso-7");
        value["request"].clone()
    };
    let Event::Answer(answer) = recorded(1) else {
        panic!()
    };
    let enable = Enable {
        name: Some("a \"name\"\nof two lines"),
        reattach: answer.remote.as_ref().and_then(|r| r.id.as_ref()),
        keep: true,
    };
    assert_eq!(
        body(&control::enable(&Asked::new(7), &enable)),
        serde_json::json!({
            "subtype": "remote_control",
            "enabled": true,
            "name": "a \"name\"\nof two lines",
            "reattach_session_id": "cse_01AbCdEfGhIjKlMnOpQrStUv",
            "keep_session_on_exit": true,
        })
    );
    let plain = serde_json::json!({"subtype": "remote_control", "enabled": true});
    assert_eq!(
        body(&control::enable(&Asked::new(7), &Enable::default())),
        plain
    );
    assert_eq!(
        body(&control::request(7, Request::RemoteControl)),
        serde_json::json!({"subtype": "remote_control", "enabled": false})
    );
}

#[test]
fn a_line_that_is_nearly_one_is_none() {
    for text in [
        r#"{"type":"system","subtype":"bridge_state","state":"Connected"}"#,
        r#"{"type":"system","subtype":"bridge_state","state":" connected"}"#,
        r#"{"type":"system","subtype":"bridge_state","state":"connected\u0000"}"#,
        r#"{"type":"system","subtype":"bridge_state","state":["connected"]}"#,
        r#"{"type":"system","subtype":"bridge_state","state":true}"#,
        r#"{"type":"system","subtype":"bridge_state"}"#,
        r#"{"type":"system","subtype":"Bridge_state","state":"connected"}"#,
        r#"{"type":"bridge_state","state":"connected"}"#,
        r#"{"type":["system"],"subtype":"bridge_state","state":"connected"}"#,
        r#"{"type":"stream_event","event":{"type":"system","subtype":"bridge_state","state":"connected"}}"#,
        r#"{"type":"command_lifecycle","command_uuid":"x","state":"queued"}"#,
        r#"{"type":"command_lifecycle","command_uuid":"00000000-0000-4000-8000-00000000000A","state":"queued"}"#,
        r#"{"type":"command_lifecycle","command_uuid":"00000000-0000-4000-8000-00000000000a ","state":"queued"}"#,
        r#"{"type":"command_lifecycle","command_uuid":"00000000-0000-4000-8000-00000000000a","state":"Queued"}"#,
        r#"{"type":"command_lifecycle","state":"queued"}"#,
        r#"{"type":"system","subtype":"command_lifecycle","command_uuid":"00000000-0000-4000-8000-00000000000a","state":"queued"}"#,
        r#"{"type":"control_request","request_id":"oso-1","request":{"subtype":"remote_control","enabled":true}}"#,
    ] {
        assert!(
            !matches!(
                line(text),
                Event::Connection(_) | Event::Lifecycle(_) | Event::Answer(_)
            ),
            "{text}"
        );
    }
    for text in [
        r#"[{"type":"system","subtype":"bridge_state","state":"connected"}]"#,
        r#"{"type":"system","subtype":"bridge_state","state":"connected"} x"#,
        r#"{"type":"system","subtype":"bridge_state","state":"connected""#,
    ] {
        assert!(matches!(line(text), Event::Unreadable { .. }), "{text}");
    }
    // A reason is cut.
    let long = serde_json::json!({"type": "system", "subtype": "bridge_state",
        "state": "failed", "detail": "é".repeat(10_000)});
    assert!(matches!(line(&long.to_string()),
        Event::Connection(bridge) if bridge.detail.len() <= MAX_NAME));
}

#[test]
fn only_an_address_and_an_id_of_claude_ais_shape_are_kept() {
    let given = |key: &str, value: &serde_json::Value| {
        let said = serde_json::json!({"type": "control_response", "response": {
            "subtype": "success", "request_id": "oso-1", "response": {key: value}}});
        match line(&said.to_string()) {
            Event::Answer(answer) => answer.remote.map(|e| *e).unwrap_or_default(),
            other => panic!("{other:?}"),
        }
    };
    for url in [
        "https://claude.ai/code/session_",
        "https://claude.ai/code/session_a/b",
        "https://claude.ai/code/session_a?x=1",
        "https://claude.ai/code/session_a\n",
        "https://claude.ai/code/session_é",
        "https://claude.ai/code?environment=",
        "http://claude.ai/code/session_a",
        "https://claude.ai.example/code/session_a",
        "https://claude.ai@example.com/code/session_a",
        "HTTPS://claude.ai/code/session_a",
        " https://claude.ai/code/session_a",
        &format!("https://claude.ai/code/session_{}", "a".repeat(65)),
    ] {
        assert_eq!(
            given("session_url", &url.into()),
            Enabled::default(),
            "{url:?}"
        );
    }
    for id in ["cse_", "cse_a-b", "cse_a b", "CSE_a", "session_a", "x"] {
        assert_eq!(
            given("bridge_session_id", &id.into()),
            Enabled::default(),
            "{id:?}"
        );
    }
    for value in [serde_json::json!(1), serde_json::json!(["cse_a"])] {
        assert_eq!(given("bridge_session_id", &value), Enabled::default());
    }
    for (key, value) in [
        (
            "connect_url",
            "https://claude.ai/code?environment=session_a",
        ),
        ("environment_id", "cse_a"),
    ] {
        assert_eq!(
            given(key, &value.into()),
            Enabled::default(),
            "{key} is never read"
        );
    }
    assert!(given("bridge_session_id", &"cse_a1".into()).id.is_some());
    assert!(
        given("session_url", &"https://claude.ai/code/session_a1".into())
            .url
            .is_some()
    );
    for value in ["\"true\"", "1", "null"] {
        let said = format!(
            r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"oso-0","response":{{"remote_control_available":{value}}}}}}}"#
        );
        assert!(
            matches!(line(&said), Event::Answer(answer) if answer.remote_available.is_none()),
            "{value}"
        );
    }
}
