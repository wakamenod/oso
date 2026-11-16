//! Tests of the search of conversations (ADR 0145): the scopes and their labels, the request,
//! rows with their labels and highlights, what `RET` gives, and the preview's request and its
//! wrapped excerpt.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;

use super::*;
use crate::buffer::Buffer;
use crate::editor::edit_tests::edit;
use crate::editor::{Excerpt, Preview, ProjectPreviewRead};
use crate::frame::FrameSize;
use crate::minibuffer::Outcome;

fn scopes() -> Vec<ConversationScope> {
    vec![
        ConversationScope::all(),
        ConversationScope::within(
            "space oso-2".into(),
            vec![PathBuf::from("/w/oso")],
            vec!["id".into()],
        ),
    ]
}

/// An editor with a frame wide enough for the preview and a search of conversations open.
fn searching(at: usize) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 160,
    });
    editor
        .open_conversation_search(scopes(), at, "session-search")
        .unwrap();
    editor
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

fn row(session: &str, message: usize, text: &str) -> ConversationRow {
    ConversationRow {
        session: session.into(),
        label: "◌ fix-login · webapp · 2d ›".into(),
        message,
        line: 1,
        column: 2,
        text: text.into(),
    }
}

fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect()
}

#[test]
fn a_search_asks_for_conversations_in_its_scope_and_never_for_files() {
    let mut editor = searching(0);
    assert_eq!(
        editor.prompt().unwrap().label(),
        "Search conversations (all):"
    );
    assert!(editor.searches_conversations());
    typing(&mut editor, "trust");
    assert!(
        editor.take_search_request().is_none(),
        "no files are searched"
    );
    let request = editor.take_conversation_search_request().unwrap();
    assert_eq!(request.scope, ConversationScope::all());
    assert!(
        request
            .query
            .matches_message(b"the worktree\ntrust record")
            .is_some()
    );
    let debug = format!("{request:?} {:?}", scopes()[1]);
    assert!(
        !debug.contains("trust") && !debug.contains("/w/oso"),
        "{debug}"
    );
    assert!(editor.take_conversation_search_request().is_none(), "once");
}

#[test]
fn the_next_scope_relabels_the_prompt_and_searches_again() {
    let mut editor = searching(0);
    typing(&mut editor, "trust");
    let first = editor.take_conversation_search_request().unwrap();
    editor.conversation_rows_found(first.generation, vec![row("a", 0, "trust")], None);
    assert!(editor.next_conversation_scope());
    assert_eq!(
        editor.prompt().unwrap().label(),
        "Search conversations (space oso-2):"
    );
    let again = editor.take_conversation_search_request().unwrap();
    assert!(again.generation > first.generation);
    assert_eq!(again.scope, scopes()[1]);
    assert!(rows(&editor).is_empty(), "the rows go");
    // Rows of the search before are dropped.
    editor.conversation_rows_found(first.generation, vec![row("a", 0, "trust")], None);
    assert!(rows(&editor).is_empty());
    assert!(editor.next_conversation_scope(), "and back to every one");
    assert_eq!(
        editor.prompt().unwrap().label(),
        "Search conversations (all):"
    );
}

#[test]
fn c_u_opens_in_the_given_scope_and_a_second_search_is_refused() {
    let mut editor = searching(1);
    assert_eq!(
        editor.prompt().unwrap().label(),
        "Search conversations (space oso-2):"
    );
    assert_eq!(
        editor.open_conversation_search(scopes(), 0, "session-search"),
        Err(PromptError::Invalid)
    );
    let mut none = Editor::with_buffer(Buffer::from_text("t", ""));
    assert_eq!(
        none.open_conversation_search(Vec::new(), 0, "session-search"),
        Err(PromptError::Invalid)
    );
}

#[test]
fn rows_show_their_label_and_ret_gives_the_session_and_keeps_the_input() {
    let mut editor = searching(0);
    typing(&mut editor, "needle");
    let request = editor.take_conversation_search_request().unwrap();
    editor.conversation_rows_found(
        request.generation,
        vec![row("6c1b5d1e", 3, "a needle here")],
        Some(WalkEnd { cut: false }),
    );
    let shown = rows(&editor);
    assert_eq!(shown, ["◌ fix-login · webapp · 2d › a needle here"]);
    let mut out = Vec::new();
    let prefix = editor.search_highlights(0, &shown[0], &mut out).unwrap();
    assert_eq!(&shown[0][..prefix], "◌ fix-login · webapp · 2d › ");
    assert_eq!(&shown[0][out[0].clone()], "needle");
    // A file's rows are not this search's.
    editor.rows_found(
        request.generation,
        vec![crate::editor::ProjectSearchRow {
            path: "a.rs".into(),
            line: 1,
            offset: 0,
            column: 0,
            text: "needle".into(),
            secret: false,
        }],
        None,
    );
    assert_eq!(rows(&editor).len(), 1);
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(text, "6c1b5d1e");
    assert_eq!(
        editor.take_conversation_hit(),
        Some(ConversationHit {
            session: "6c1b5d1e".into(),
            message: 3,
        })
    );
    assert_eq!(editor.take_search_hit(), None);
}

#[test]
fn the_preview_asks_for_the_rows_conversation_once_and_shows_what_came() {
    let mut editor = searching(0);
    typing(&mut editor, "needle");
    let request = editor.take_conversation_search_request().unwrap();
    editor.conversation_rows_found(request.generation, vec![row("abc", 5, "needle")], None);
    assert!(editor.take_preview_request().is_none(), "no file is read");
    let preview = editor.take_conversation_preview_request().unwrap();
    assert_eq!(
        (
            &*preview.session,
            preview.message,
            preview.line,
            preview.column
        ),
        ("abc", 5, 1, 2)
    );
    assert!(preview.rows > 0 && preview.columns > 0);
    assert!(
        editor.take_conversation_preview_request().is_none(),
        "once a row"
    );
    let excerpt = Excerpt::wrapped("› find it\n  the needle is here", preview.columns, (1, 6));
    editor.preview_read(preview.generation, ProjectPreviewRead::Text(excerpt));
    assert!(matches!(editor.preview(), Some(Preview::Excerpt { .. })));
}

#[test]
fn a_scope_holds_its_sessions_and_what_ran_under_its_roots() {
    let scope = &scopes()[1];
    assert!(scope.holds("id", &[]));
    assert!(scope.holds("other", &[PathBuf::from("/w/oso/.claude/worktrees/x")]));
    assert!(!scope.holds("other", &[PathBuf::from("/w/osopher")]));
    assert!(!scope.holds("other", &[]));
    assert!(ConversationScope::all().holds("any", &[]));
}
