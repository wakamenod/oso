//! What a native session's stream comes to in the app for Remote Control (ADR 0181): the canary,
//! the request's answer, and a message marked before the rows read it. The rules are
//! `sessions::native::remote`'s, tested there.
//!
//! Must not depend on: a window or a process.

use std::time::Instant;

use oso_core::editor::Editor;

use super::super::Native;
use super::super::markdown::tests::native;
use super::{Follow, control_event};
use crate::sessions::native::control::Control;
use crate::sessions::native::mode::ModeState;
use crate::sessions::native::remote::{Phase, Remote};
use crate::sessions::native::wire::remote::Came;
use crate::sessions::native::wire::{Event, parse};

fn session() -> Native {
    let mut editor = Editor::default();
    let mut native = native(editor.add_native_session_buffer("*claude: a*"));
    native.control = Some(Control::started(Instant::now()));
    native
}

fn read(native: &mut Native, line: &str) -> (Event, Vec<Follow>) {
    let (mut event, mut follows) = (parse(line.as_bytes()), Vec::new());
    control_event(native, &mut event, &mut follows, true);
    (event, follows)
}

fn initialize(available: bool) -> String {
    format!(
        r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"oso-0","response":{{"remote_control_available":{available}}}}}}}"#
    )
}

const CONNECTED: &str = r#"{"type":"system","subtype":"bridge_state","state":"connected"}"#;
const REMOTE: &str = r#"{"type":"user","isReplay":true,"uuid":"u","origin":{"kind":"human"},"message":{"content":"Go on"}}"#;

#[test]
fn the_canary_is_said_once_and_turns_nothing_on() {
    let mut native = session();
    assert_eq!(read(&mut native, &initialize(false)).1, Vec::new());
    assert_eq!(read(&mut native, CONNECTED).1, vec![Follow::Canary]);
    assert_eq!(read(&mut native, CONNECTED).1, Vec::new());
    assert!(!native.remote.marked());

    let mut native = session();
    assert_eq!(read(&mut native, &initialize(true)).1, vec![Follow::Canary]);
    // Only the first answer is one.
    let mut native = session();
    read(&mut native, &initialize(false));
    assert_eq!(read(&mut native, &initialize(true)).1, Vec::new());
    let (event, _) = read(&mut native, REMOTE);
    assert!(matches!(event, Event::User(user) if user.came == Came::AsSaid));
}

#[test]
fn an_answer_reaches_the_request_that_waits_and_a_message_is_marked() {
    let mut native = session();
    native.remote = Remote::unblocked();
    read(&mut native, &initialize(true));
    let control = native.control.as_mut().unwrap();
    let id = native
        .remote
        .begin(control, 7, Instant::now())
        .unwrap()
        .id();
    let answer = format!(
        r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"oso-{id}","response":{{"session_url":"https://claude.ai/code/session_a"}}}}}}"#
    );
    assert_eq!(read(&mut native, &answer).1, Vec::new());
    let url = native.remote.enabled().url.as_ref();
    assert_eq!(
        url.map(|url| url.as_str()),
        Some("https://claude.ai/code/session_a")
    );
    assert_eq!(native.remote.phase(), &Phase::Connecting);
    assert_eq!(read(&mut native, CONNECTED).1, Vec::new());
    assert_eq!(native.remote.phase(), &Phase::On);
    let (event, _) = read(&mut native, REMOTE);
    assert!(matches!(event, Event::User(user) if user.came == Came::Remote));
    // A batch read as its control layer went is judged too.
    native.control = None;
    let (event, _) = read(&mut native, REMOTE);
    assert!(matches!(event, Event::User(user) if user.came == Came::Remote));
}

/// The app tells the mode whether it may be from elsewhere, as `initialize` and the stream say it.
#[test]
fn a_mode_from_elsewhere_is_not_carried_where_the_process_may_be_connected() {
    use crate::channel::protocol::mode::PermissionMode::{Auto, Default as Asks};
    let mode = |mode: &str| {
        parse(
            format!(r#"{{"type":"system","subtype":"status","permissionMode":"{mode}"}}"#)
                .as_bytes(),
        )
    };
    for (remote, carried) in [(Remote::unblocked(), Asks), (Remote::default(), Auto)] {
        let mut native = session();
        native.mode = ModeState::restarted(Some(Auto));
        native.remote = remote;
        for said in ["auto", "default", "auto"] {
            super::super::mode::mode_event(&mut native, &mode(said));
        }
        assert_eq!(native.mode.carried(), Some(carried));
    }
    let mut native = session();
    native.mode = ModeState::restarted(Some(Auto));
    native.remote = Remote::unblocked();
    let answer = r#"{"type":"control_response","response":{"subtype":"success","request_id":"oso-0","response":{"current_permission_mode":"plan"}}}"#;
    read(&mut native, answer);
    assert_eq!(native.mode.carried(), Some(Asks));
}
