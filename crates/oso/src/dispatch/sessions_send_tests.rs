//! Sending to agent sessions through a dispatcher (ADR 0121, step 5): `C-x a m` reads a
//! session, then a note unless a region shows, and asks the host to paste what it read;
//! `C-x a @` inserts a session's name; `C-x a f` asks for a message's sender.
//!
//! Must not depend on: `app` or winit; these drive a dispatcher and an editor alone.

use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::Buffer;
use oso_core::command::Reach;
use oso_core::complete::Candidate;
use oso_core::editor::{Editor, SendExcerpt, SendPart, SendTo, SessionRequest};

use super::Dispatcher;
use super::args::Given;
use super::motion_tests::send;

const API: &str = "*claude: api*";

fn editor(buffer: Buffer) -> Editor {
    let mut editor = Editor::with_buffer(buffer);
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 40,
        columns: 100,
    });
    let sessions: Arc<[Candidate]> = Arc::from([
        Candidate::new(API, "", "idle · web"),
        Candidate::new("*claude: fix*", "", "working · oso"),
    ]);
    editor.set_listed("session-send", sessions);
    editor.set_listed(
        "session-mention",
        Arc::from([Candidate::new("api", "", "/src/web")]),
    );
    editor
}

fn notes() -> Editor {
    editor(Buffer::from_text("notes.md", "one\ntwo\nthree\n"))
}

fn sent(to: &str, parts: Vec<SendPart>) -> SessionRequest {
    SessionRequest::Send {
        to: SendTo::Named(to.to_owned()),
        parts,
    }
}

fn lines(first: usize, last: usize, text: Option<&str>) -> SendPart {
    SendPart::Excerpt(SendExcerpt {
        path: None,
        buffer: "notes.md".to_owned(),
        first,
        last,
        text: text.map(str::to_owned),
    })
}

fn messages(editor: &Editor) -> String {
    let id = editor.messages_buffer();
    editor.buffer(id).unwrap().text().to_string()
}

#[test]
fn a_region_goes_with_its_lines_and_text_and_no_note_is_asked() {
    let (mut d, mut editor) = (Dispatcher::new(), notes());
    // The region ends at the start of the third line, which it leaves out.
    send(&mut d, &mut editor, "C-SPC C-n C-n C-x a m");
    assert!(
        editor
            .prompt()
            .unwrap()
            .label()
            .starts_with("Send to session:")
    );
    send(&mut d, &mut editor, "RET");
    assert!(editor.prompt().is_none(), "no note is read");
    assert_eq!(
        editor.take_session_requests(),
        [sent(API, vec![lines(1, 2, Some("one\ntwo\n"))])]
    );
}

#[test]
fn with_no_region_a_note_is_read_and_sent() {
    let (mut d, mut editor) = (Dispatcher::new(), notes());
    send(&mut d, &mut editor, "C-x a m f i x RET");
    assert!(editor.prompt().unwrap().label().starts_with("Note:"));
    send(&mut d, &mut editor, "w h y SPC n o t ? RET");
    assert_eq!(
        editor.take_session_requests(),
        [sent(
            "*claude: fix*",
            vec![SendPart::Note("why not?".to_owned())]
        )]
    );
}

#[test]
fn an_empty_note_or_c_g_sends_nothing() {
    let (mut d, mut editor) = (Dispatcher::new(), notes());
    send(&mut d, &mut editor, "C-x a m RET RET");
    assert!(editor.take_session_requests().is_empty());
    assert!(messages(&editor).contains("Nothing to send"));
    send(&mut d, &mut editor, "C-x a m RET a C-g");
    send(&mut d, &mut editor, "C-x a m C-g");
    assert!(editor.take_session_requests().is_empty());
}

#[test]
fn c_u_sends_the_place_only() {
    let (mut d, mut editor) = (Dispatcher::new(), notes());
    send(&mut d, &mut editor, "C-n C-SPC C-n C-f C-u C-x a m RET");
    assert_eq!(
        editor.take_session_requests(),
        [sent(API, vec![lines(2, 3, None)])]
    );
    // With no region: the line at point, then the note, if any.
    send(&mut d, &mut editor, "C-g C-u C-x a m RET l o o k RET");
    assert_eq!(
        editor.take_session_requests(),
        [sent(
            API,
            vec![lines(3, 3, None), SendPart::Note("look".to_owned())]
        )]
    );
    send(&mut d, &mut editor, "C-u C-x a m RET RET");
    assert_eq!(
        editor.take_session_requests(),
        [sent(API, vec![lines(3, 3, None)])]
    );
}

#[test]
fn a_buffer_that_may_hold_secrets_sends_its_place_only() {
    let path = PathBuf::from("/work/repo/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.finish_empty();
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    assert!(buffer.is_secret());
    let (mut d, mut editor) = (Dispatcher::new(), editor(buffer));
    // Near a secret the float takes only what is typed, not its first candidate.
    send(&mut d, &mut editor, "k e y RET C-SPC M-< C-x a m a p i RET");
    let SessionRequest::Send { parts, .. } = &editor.take_session_requests()[0] else {
        panic!("no send");
    };
    let [SendPart::Excerpt(excerpt)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert_eq!(excerpt.path.as_deref(), Some(path.as_path()));
    assert_eq!((excerpt.first, excerpt.last, &excerpt.text), (1, 1, &None));
}

#[test]
fn a_region_past_256_kib_is_refused_with_the_place_only_way() {
    let text = "x".repeat(300 << 10);
    let (mut d, mut editor) = (Dispatcher::new(), editor(Buffer::from_text("big", &text)));
    send(&mut d, &mut editor, "C-SPC M-> C-x a m RET");
    assert!(editor.take_session_requests().is_empty());
    assert!(
        messages(&editor)
            .contains("The region is 300 KiB; at most 256 KiB goes to a session. C-u C-x a m"),
        "{}",
        messages(&editor)
    );
    send(&mut d, &mut editor, "C-u C-x a m RET");
    assert_eq!(editor.take_session_requests().len(), 1);
}

#[test]
fn each_region_of_several_cursors_is_one_excerpt_in_buffer_order() {
    let buffer = Buffer::from_text("notes.md", "ab\nxx\nab\n");
    let (mut d, mut editor) = (Dispatcher::new(), editor(buffer));
    send(&mut d, &mut editor, "C-SPC C-f C-f C-> C-x a m RET");
    assert_eq!(
        editor.take_session_requests(),
        [sent(
            API,
            vec![lines(1, 1, Some("ab")), lines(3, 3, Some("ab"))]
        )]
    );
}

#[test]
fn a_mention_inserts_the_name_at_each_cursor() {
    let (mut d, mut editor) = (Dispatcher::new(), notes());
    send(&mut d, &mut editor, "C-e C-x a @ RET");
    assert_eq!(
        editor.current().1.text().to_string(),
        "one@api\ntwo\nthree\n"
    );
    assert!(editor.take_session_requests().is_empty());
}

#[test]
fn c_x_a_f_asks_for_the_sender_of_this_sessions_newest_message() {
    let (mut d, mut editor) = (Dispatcher::new(), notes());
    let buffer = editor.window().buffer();
    send(&mut d, &mut editor, "C-x a f");
    assert_eq!(
        editor.take_session_requests(),
        [SessionRequest::FollowMessage(buffer)]
    );
}

#[test]
fn the_agent_channel_cannot_send_or_mention() {
    let (mut d, mut editor) = (Dispatcher::new(), notes());
    for (name, given) in [
        ("session-send", Given::Nothing),
        ("session-send", Given::Text(API)),
        ("session-mention", Given::Text("api")),
    ] {
        let refused = match d.run_for_agent(&mut editor, name, given, Reach::Change) {
            Err(_) => true,
            Ok((_, result)) => result.is_err(),
        };
        assert!(refused, "{name}");
    }
    assert!(editor.take_session_requests().is_empty());
    assert_eq!(editor.current().1.text().to_string(), "one\ntwo\nthree\n");
}
