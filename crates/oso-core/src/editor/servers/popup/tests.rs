//! Tests of the completion popup (ADR 0138, step 9): a stale answer, the word it completes,
//! matching as the word is typed, closing once the cursor leaves it, moving with wrap, and
//! inserting at one cursor and at those that share the word.
//!
//! Must not depend on: the host or its servers; these drive the editor alone.

use super::*;
use crate::buffer::Buffer;
use crate::editor::edit_tests::edit;
use crate::frame::FrameSize;
use crate::selection::{Selection, SelectionSet};

/// An editor showing `text` with the cursor at `at`, wide enough for a popup.
fn editing(text: &str, at: usize) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", text));
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 80,
    });
    editor.current_mut().0.selections = SelectionSet::new(at);
    editor
}

fn item(label: &str, range: Option<Range<usize>>) -> CompletionPopupItem {
    CompletionPopupItem {
        label: label.into(),
        annotation: "function".into(),
        filter: None,
        text: label.into(),
        range,
        extra: Vec::new(),
    }
}

fn answer(editor: &Editor, items: Vec<CompletionPopupItem>) -> CompletionPopupAnswer {
    let at = editor.ask_at();
    CompletionPopupAnswer {
        serial: 1,
        incomplete: false,
        at,
        revision: at.revision,
        items,
    }
}

fn labels(editor: &Editor) -> Vec<String> {
    let popup = editor.completion_popup().unwrap();
    popup.rows().map(|(_, row)| row.text().to_owned()).collect()
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

#[test]
fn an_answer_opens_the_popup_over_the_word_matched_and_a_stale_one_does_not() {
    let mut editor = editing("let x = fo", 10);
    let items = vec![item("foo", None), item("bar", None), item("format", None)];
    let stale = CompletionPopupAnswer {
        serial: 1,
        incomplete: false,
        at: AskAt {
            offset: 3,
            ..editor.ask_at()
        },
        revision: 0,
        items: Vec::new(),
    };
    assert_eq!(editor.open_completion_popup(stale), Err(None));
    assert!(editor.completion_popup().is_none());
    assert_eq!(
        editor.open_completion_popup(answer(&editor, Vec::new())),
        Err(Some("No completions"))
    );
    editor
        .open_completion_popup(answer(&editor, items))
        .unwrap();
    assert_eq!(labels(&editor), ["foo", "format"]);
    assert!(!editor.completion_popup().unwrap().moved());
    assert!(!format!("{:?}", editor.completion_popup().unwrap()).contains("foo"));
}

#[test]
fn typing_narrows_and_leaving_the_word_closes_it() {
    let mut editor = editing("fo", 2);
    let items = vec![item("foo", None), item("format", None)];
    editor
        .open_completion_popup(answer(&editor, items))
        .unwrap();
    let id = editor.window().buffer();
    editor.apply_edits(id, &[edit(2..2, "r")]).unwrap();
    editor.follow_completion_popup();
    assert_eq!(labels(&editor), ["format"]);
    editor.apply_edits(id, &[edit(3..3, " ")]).unwrap();
    editor.follow_completion_popup();
    assert!(
        editor.completion_popup().is_none(),
        "a blank is not in the word"
    );
}

#[test]
fn the_selection_wraps_and_moving_it_is_kept() {
    let mut editor = editing("", 0);
    let items = vec![item("a", None), item("b", None), item("c", None)];
    editor
        .open_completion_popup(answer(&editor, items))
        .unwrap();
    editor.completion_popup_move(-1);
    let selected = |editor: &Editor| {
        let popup = editor.completion_popup().unwrap();
        popup.rows().position(|(selected, _)| selected)
    };
    assert_eq!(selected(&editor), Some(2), "from the first to the last");
    assert!(editor.completion_popup().unwrap().moved());
    editor.completion_popup_move(1);
    assert_eq!(selected(&editor), Some(0));
}

#[test]
fn inserting_replaces_the_word_and_the_server_s_range_past_it() {
    let mut editor = editing("x.fo_bar", 4);
    // The server replaces `fo_bar` whole.
    let items = vec![item("format", Some(2..8))];
    editor
        .open_completion_popup(answer(&editor, items))
        .unwrap();
    editor.completion_popup_insert().unwrap();
    assert_eq!(text(&editor), "x.format");
    assert_eq!(editor.window().selections.primary().cursor, 8);
    assert!(editor.completion_popup().is_none());
}

#[test]
fn the_item_goes_in_at_each_cursor_whose_word_is_the_primary_s() {
    let mut editor = editing("fo\nfo\nxfo\n", 2);
    let cursors = vec![Selection::at(2), Selection::at(5), Selection::at(9)];
    editor.current_mut().0.selections = SelectionSet::from_parts(cursors, 0).unwrap();
    editor
        .open_completion_popup(answer(&editor, vec![item("foo", None)]))
        .unwrap();
    editor.completion_popup_insert().unwrap();
    assert_eq!(
        text(&editor),
        "foo\nfoo\nxfo\n",
        "not where the word is longer"
    );
}

#[test]
fn the_popup_goes_under_the_word_s_start() {
    let mut editor = editing("    let x = fo", 14);
    editor
        .open_completion_popup(answer(
            &editor,
            vec![item("foo", None), item("format", None)],
        ))
        .unwrap();
    let mut screen = Screen::default();
    editor.screen(&mut screen);
    let regions = editor.completion_popup_regions(&screen).unwrap();
    let text = editor.tiled_regions().text;
    assert_eq!(regions.outer.row, text.row + 1);
    assert_eq!(regions.inner.column, text.column + 12);
    assert_eq!(regions.inner.rows, 2);
    assert_eq!(regions.inner.columns, "format".len() + 2 + "function".len());
}

#[test]
fn a_range_off_the_cursor_s_line_or_far_from_it_is_not_followed() {
    let mut editor = editing("let a = 1;\nfo\n", 13);
    // From the top of the file to the cursor: only the word is replaced.
    editor
        .open_completion_popup(answer(&editor, vec![item("foo", Some(0..13))]))
        .unwrap();
    editor.completion_popup_insert().unwrap();
    assert_eq!(text(&editor), "let a = 1;\nfoo\n");
    // Past the cursor, across the line's end.
    let mut editor = editing("fo\nbar\n", 2);
    editor
        .open_completion_popup(answer(&editor, vec![item("foo", Some(0..6))]))
        .unwrap();
    editor.completion_popup_insert().unwrap();
    assert_eq!(text(&editor), "foo\nbar\n");
}

#[test]
fn a_buffer_that_may_hold_secrets_gets_no_popup() {
    let path = std::path::PathBuf::from("/tmp/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(&[path]));
    let mut editor = Editor::with_buffer(buffer);
    assert!(editor.current().1.is_secret());
    let opened = editor.open_completion_popup(answer(&editor, vec![item("TOKEN", None)]));
    assert_eq!(opened, Err(None));
    assert!(editor.completion_popup().is_none());
}

#[test]
fn with_no_word_typed_the_item_goes_in_at_the_primary_cursor_only() {
    let mut editor = editing("a.\nb.\n", 2);
    let cursors = vec![Selection::at(2), Selection::at(5)];
    editor.current_mut().0.selections = SelectionSet::from_parts(cursors, 0).unwrap();
    editor
        .open_completion_popup(answer(&editor, vec![item("len", None)]))
        .unwrap();
    editor.completion_popup_insert().unwrap();
    assert_eq!(text(&editor), "a.len\nb.\n");
}

#[test]
fn a_prompt_closes_the_popup() {
    let mut editor = editing("fo", 2);
    editor
        .open_completion_popup(answer(&editor, vec![item("foo", None)]))
        .unwrap();
    editor
        .open_prompt(
            "Go to line:",
            crate::minibuffer::Kind::Text { history: None },
        )
        .unwrap();
    assert!(editor.completion_popup().is_none());
}

#[test]
fn on_a_wrapped_line_the_popup_hangs_from_the_cursor_s_row() {
    // A narrow frame: the word starts on one screen row and the cursor is on the next.
    let text = "abcdefghijklmnopqrst".to_owned();
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", &text));
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 12,
    });
    let id = editor.window().buffer();
    editor.set_wraps(id, true);
    editor.current_mut().0.selections = SelectionSet::new(text.len());
    editor
        .open_completion_popup(answer(&editor, vec![item("abcdefghijklmnopqrstuv", None)]))
        .unwrap();
    let mut screen = Screen::default();
    editor.screen(&mut screen);
    let regions = editor.completion_popup_regions(&screen).unwrap();
    let display = editor.current().1.display();
    let cursor = display.offset_to_display_point(text.len());
    let start = display.offset_to_display_point(0);
    let cursor_row = screen.row_of(cursor).unwrap();
    assert_ne!(screen.row_of(start), Some(cursor_row), "the word wraps");
    let area = editor.tiled_regions().text;
    assert!(regions.outer.row > area.row + cursor_row, "{regions:?}");
}
