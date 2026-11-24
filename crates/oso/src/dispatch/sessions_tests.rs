//! The keys of agent sessions through a dispatcher (ADR 0121, step 3): `C-c a` and a key asks
//! the host, the switch floats complete over what the host listed, and `*Spaces*`'s own keys
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

/// An editor showing `*Spaces*` with `text`, point at its start.
fn listing(text: &str) -> Editor {
    let mut editor = editor();
    let list = editor.add_internal_buffer("*Spaces*", Mode::Sessions);
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
fn c_c_a_keys_ask_the_host() {
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    let buffer = editor.window().buffer();
    send(
        &mut d,
        &mut editor,
        "C-c a s C-c a n C-c a d C-c a z C-c a e",
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
    send(&mut d, &mut editor, "C-c a b");
    let prompt = editor.prompt().expect("the float");
    assert!(prompt.label().starts_with("Session:"), "{}", prompt.label());
    send(&mut d, &mut editor, "f i x RET");
    editor.set_listed(
        "spaces",
        Arc::from([Candidate::new("oso/fix-1", "", "/src/oso-wt/fix-1")]),
    );
    send(&mut d, &mut editor, "C-c a j RET");
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::Switch("*claude: fix*".to_owned()),
            SessionRequest::Space("oso/fix-1".to_owned()),
        ]
    );
}

#[test]
fn space_close_offers_the_space_in_front_first_and_closes_the_one_chosen() {
    use oso_core::editor::SpaceRequest;
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    // As the host gives them: the one in front first, though it is the longest.
    let spaces: Arc<[Candidate]> = Arc::from([
        Candidate::new("the one in front", "", "oso"),
        Candidate::new("api", "", "web"),
        Candidate::new("fix", "", "oso"),
    ]);
    editor.set_listed("spaces-open", spaces);
    let closed = |editor: &mut Editor, name: &str| {
        assert_eq!(
            editor.take_session_requests(),
            [SessionRequest::Spaces(SpaceRequest::Close(name.to_owned()))]
        );
    };
    send(&mut d, &mut editor, "C-c a S c");
    let prompt = editor.prompt().expect("the list");
    assert!(
        prompt.label().starts_with("Close space:"),
        "{}",
        prompt.label()
    );
    send(&mut d, &mut editor, "f i x RET");
    closed(&mut editor, "fix");
    // RET alone, though another was closed before.
    send(&mut d, &mut editor, "C-c a S c RET");
    closed(&mut editor, "the one in front");
    // From `M-x` too.
    let typed: Vec<String> = "space-close".chars().map(String::from).collect();
    send(
        &mut d,
        &mut editor,
        &format!("M-x {} RET RET", typed.join(" ")),
    );
    closed(&mut editor, "the one in front");
    // Only a listed space is an answer, and `C-g` closes none.
    send(&mut d, &mut editor, "C-c a S c z z RET");
    assert!(
        editor.prompt().is_some(),
        "nothing matches: the prompt stays"
    );
    send(&mut d, &mut editor, "C-g C-c a S c C-g");
    assert!(editor.prompt().is_none());
    assert!(editor.take_session_requests().is_empty());
}

#[test]
fn with_no_open_space_a_close_asks_nothing() {
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    send(&mut d, &mut editor, "C-c a S c RET");
    assert!(editor.take_session_requests().is_empty());
}

#[test]
fn a_switch_over_nothing_asks_nothing() {
    let (mut d, mut editor) = (Dispatcher::new(), editor());
    send(&mut d, &mut editor, "C-c a b RET");
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
    send(&mut d, &mut editor, "C-c a a RET");
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
    send(&mut d, &mut editor, "C-c a b RET");
    assert_eq!(
        editor.take_session_requests(),
        [SessionRequest::Switch("old".to_owned())]
    );
}

#[test]
fn session_searchs_prompt_has_its_own_keys_over_completions() {
    let mut d = Dispatcher::new();
    let mut editor = editor();
    send(&mut d, &mut editor, "C-c a /");
    assert_eq!(
        editor.prompt().map(|prompt| prompt.label().to_owned()),
        Some("Search conversations (all):".to_owned())
    );
    send(&mut d, &mut editor, "n e e d l e");
    let request = editor.take_conversation_search_request().unwrap();
    let row = oso_core::editor::ConversationRow {
        session: "6c1b5d1e-2f3a-4b5c-8d9e-0f1a2b3c4d5e".into(),
        label: "◌ x · y · 1m ›".into(),
        message: 2,
        line: 0,
        column: 0,
        text: "needle".into(),
    };
    editor.conversation_rows_found(request.generation, vec![row], None);
    // With only every conversation's scope, `C-c C-s` stays in it.
    send(&mut d, &mut editor, "C-c C-s");
    assert!(editor.searches_conversations());
    // `M-RET` forks, in place of completion's exit with the input.
    send(&mut d, &mut editor, "M-RET");
    assert!(editor.prompt().is_none());
    assert_eq!(
        editor.take_session_requests(),
        [SessionRequest::ForkConversation {
            session: "6c1b5d1e-2f3a-4b5c-8d9e-0f1a2b3c4d5e".into(),
        }]
    );
    // Elsewhere, `C-c C-s` is no prompt's key.
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, "C-c C-s");
    assert!(
        editor
            .prompt()
            .is_some_and(|prompt| prompt.label() == "M-x")
    );
}
