//! The keys of agent sessions through a dispatcher (ADR 0121, step 3): `C-x a` and a key asks
//! the host, the switch floats complete over what the host listed, and `*Sessions*`'s own keys
//! act on the row point is on.
//!
//! Must not depend on: `app` or winit; these drive a dispatcher and an editor alone.

use std::sync::Arc;

use oso_core::buffer::Mode;
use oso_core::complete::Candidate;
use oso_core::editor::{Editor, RowAction, SessionRequest};

use super::Dispatcher;
use super::motion_tests::send;

fn editor() -> Editor {
    let mut editor = Editor::default();
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 40,
        columns: 100,
    });
    editor
}

/// An editor showing `*Sessions*` with `text`, point at its start.
fn listing(text: &str) -> Editor {
    let mut editor = editor();
    let list = editor.add_internal_buffer("*Sessions*", Mode::Sessions);
    editor.set_internal_text(list, text);
    editor.show(list);
    editor
}

const LIST: &str = "\
▾ Needs input 2
⚠ fix-tests                 3m
  oso · main
  Do you want to proceed?
⚠ docs                      1m
  oso · docs
  Which colour?
▾ Working 1
▶ api                       0s
  web · main
  Nucleating…
";

#[test]
fn c_x_a_keys_ask_the_host() {
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    let buffer = editor.window().buffer();
    send(
        &mut d,
        &mut editor,
        "C-x a s C-x a n C-x a d C-x a z C-x a e",
    );
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::ShowList,
            SessionRequest::NextWaiting,
            SessionRequest::NextDone,
            SessionRequest::Zoom,
            SessionRequest::Explain(buffer),
        ]
    );
}

#[test]
fn the_switch_floats_complete_over_what_the_host_listed() {
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    let sessions: Arc<[Candidate]> = Arc::from([
        Candidate::new("*claude: api*", "", "working · web · main"),
        Candidate::new("*claude: fix*", "", "needs input · oso · main"),
    ]);
    editor.set_listed("sessions", sessions);
    send(&mut d, &mut editor, "C-x a b");
    let prompt = editor.prompt().expect("the float");
    assert!(prompt.label().starts_with("Session:"), "{}", prompt.label());
    send(&mut d, &mut editor, "f i x RET");
    editor.set_listed(
        "spaces",
        Arc::from([Candidate::new("oso/fix-1", "", "/src/oso-wt/fix-1")]),
    );
    send(&mut d, &mut editor, "C-x a j RET");
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::Switch("*claude: fix*".to_owned()),
            SessionRequest::Space("oso/fix-1".to_owned()),
        ]
    );
}

#[test]
fn a_switch_over_nothing_asks_nothing() {
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    send(&mut d, &mut editor, "C-x a b RET");
    assert!(editor.take_session_requests().is_empty());
}

#[test]
fn the_lists_keys_act_on_the_row_point_is_on() {
    let (mut d, mut editor) = (Dispatcher::new(), listing(LIST));
    send(&mut d, &mut editor, "n");
    assert_eq!(editor.cursor_line(), 1, "the first session");
    send(&mut d, &mut editor, "n n");
    assert_eq!(
        editor.cursor_line(),
        7,
        "past a session's lines, to a group"
    );
    send(&mut d, &mut editor, "p");
    assert_eq!(editor.cursor_line(), 4);
    send(&mut d, &mut editor, "RET TAB P A r k e g s");
    let row = |action| SessionRequest::Row { line: 4, action };
    assert_eq!(
        editor.take_session_requests(),
        [
            row(RowAction::Show),
            row(RowAction::Fold),
            row(RowAction::Pin),
            row(RowAction::Archive),
            row(RowAction::Resume),
            row(RowAction::Stop),
            row(RowAction::Explain),
            SessionRequest::Refresh,
            SessionRequest::Regroup,
        ]
    );
    send(&mut d, &mut editor, "M-> p p p p p p");
    assert_eq!(editor.cursor_line(), 0, "stops at the first row");
    send(&mut d, &mut editor, "C-u 9 n");
    assert_eq!(editor.cursor_line(), 8, "stops at the last row");
}

#[test]
fn the_adopt_float_keeps_the_order_the_host_gave() {
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    // Newest first: a length or a history must not reorder them.
    let adoptable: Arc<[Candidate]> = Arc::from([
        Candidate::new("a-longer-newest-name", "", "1m ago · /p/oso"),
        Candidate::new("old", "", "3d ago · /p/oso"),
    ]);
    editor.set_listed(crate::commands::agent_sessions::ADOPT_LIST, adoptable);
    send(&mut d, &mut editor, "C-x a a RET");
    assert_eq!(
        editor.take_session_requests(),
        [SessionRequest::Adopt("a-longer-newest-name".to_owned())]
    );
    // The same candidates in another list are sorted as completion sorts them.
    editor.set_listed(
        "sessions",
        Arc::from([
            Candidate::new("a-longer-newest-name", "", ""),
            Candidate::new("old", "", ""),
        ]),
    );
    send(&mut d, &mut editor, "C-x a b RET");
    assert_eq!(
        editor.take_session_requests(),
        [SessionRequest::Switch("old".to_owned())]
    );
}
