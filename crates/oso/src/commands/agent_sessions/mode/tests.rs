//! Tests of the commands on the session in front from a window docked under its transcript, as
//! its composer, card and strip are (ADR 0170, "Session commands from a docked window"): they
//! act on the transcript's session, as if pressed in the transcript.
//!
//! Must not depend on: the host; the requests are only looked at.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::command::Args;
use oso_core::editor::{Editor, RowAction, SessionRequest};
use oso_core::frame::FrameSize;
use oso_core::window::DockSlot;

use super::super::super::tests::run;

/// An editor showing a transcript, which hosts a session if `hosts`, with a buffer docked under
/// it in `slot`, that docked window selected; and the transcript's buffer.
fn docked_in(slot: DockSlot, hosts: bool) -> (Editor, BufferId) {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", "one\ntwo\n"));
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 80,
    });
    let host = editor.add_native_session_buffer("*claude: s*");
    editor.show(host);
    editor.set_hosts_session(host, hosts);
    let docked = editor.add_buffer(Buffer::from_text("*compose: s*", "draft"));
    editor.set_dock(host, slot, Some(docked));
    let window = editor.dock_window(host, slot).expect("a docked window");
    editor.select_window(window);
    assert_eq!(editor.window().buffer(), docked);
    (editor, host)
}

#[test]
fn from_the_composer_the_session_commands_name_the_transcript() {
    let (mut editor, host) = docked_in(DockSlot::Composer, true);
    for name in [
        "session-resume",
        "session-fork",
        "session-switch-mode",
        "session-explain",
        "session-follow-message",
        "session-pin",
        "session-archive",
        "session-stop",
        "session-rename",
    ] {
        assert_eq!(run(&mut editor, name, Args::NONE).0, Ok(()), "{name}");
    }
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::Resume(host),
            SessionRequest::Fork(host),
            SessionRequest::SwitchMode(host),
            SessionRequest::Explain(host),
            SessionRequest::FollowMessage(host),
            SessionRequest::OnBuffer(host, RowAction::Pin),
            SessionRequest::OnBuffer(host, RowAction::Archive),
            SessionRequest::OnBuffer(host, RowAction::Stop),
            SessionRequest::OnBuffer(host, RowAction::Rename),
        ]
    );
}

#[test]
fn from_the_card_and_the_strip_they_name_the_transcript_too() {
    for slot in [DockSlot::Card, DockSlot::Strip] {
        let (mut editor, host) = docked_in(slot, true);
        assert_eq!(run(&mut editor, "session-pin", Args::NONE).0, Ok(()));
        assert_eq!(run(&mut editor, "session-resume", Args::NONE).0, Ok(()));
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::OnBuffer(host, RowAction::Pin),
                SessionRequest::Resume(host),
            ],
            "{slot:?}"
        );
    }
}

#[test]
fn a_window_docked_under_a_buffer_that_hosts_no_session_is_refused_as_that_buffer() {
    let (mut editor, _) = docked_in(DockSlot::Card, false);
    for name in [
        "session-pin",
        "session-archive",
        "session-stop",
        "session-rename",
    ] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert!(result.is_err(), "{name}");
    }
    assert!(editor.take_session_requests().is_empty());
}
