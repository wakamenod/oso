//! The tests of the editor's part of dictation: requests, the target, and the provisional
//! spans with their fix (ADR 0175).
//!
//! Must not depend on: the host's threads or processes; these drive the editor alone.

use super::*;
use crate::buffer::Buffer;
use crate::selection::{Selection, SelectionSet};
use crate::window::ViewSize;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", text));
    editor.set_view_size(ViewSize::new(10, 40));
    editor
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn undo(editor: &mut Editor) {
    editor.begin_command();
    editor.begin_undo_group();
    editor.undo().unwrap();
    editor.end_undo_group();
    editor.end_command();
}

/// Types `text` at offset `at` of the selected window's buffer, as a person's edit does.
fn type_at(editor: &mut Editor, at: usize, text: &str) {
    let id = editor.window().buffer();
    let edit = Replacement {
        range: at..at,
        text,
    };
    editor.apply_edits(id, &[edit]).unwrap();
}

fn span_of(placed: DictationPlaced) -> DictationSpan {
    match placed {
        DictationPlaced::Typed { span: Some(span) } => span,
        other => panic!("no span: {other:?}"),
    }
}

fn ranges(editor: &Editor, span: DictationSpan) -> Vec<Range<usize>> {
    editor.dictation_span_at(span).unwrap().ranges.clone()
}

fn secret_editor() -> Editor {
    let mut editor = editor("x");
    let mut buffer = Buffer::from_text("env", "x");
    buffer.judge_copy(
        &[std::path::PathBuf::from("/tmp/.env")],
        crate::secret::Verdict::SecretName,
    );
    let secret = editor.add_buffer(buffer);
    editor.show(secret);
    assert!(editor.current().1.is_secret());
    editor
}

#[test]
fn requests_go_to_the_host_once_in_order() {
    let mut editor = Editor::default();
    editor.request_dictation(DictationRequest::Toggle { prefix: true });
    editor.request_dictation(DictationRequest::Cancel);
    assert_eq!(
        editor.take_dictation_requests(),
        [
            DictationRequest::Toggle { prefix: true },
            DictationRequest::Cancel
        ]
    );
    assert!(editor.take_dictation_requests().is_empty());
    assert_eq!(editor.dictation_mark(), DictationMark::Off);
}

#[test]
fn the_text_goes_in_at_each_cursor_from_the_start_in_one_undo_step() {
    let mut editor = editor("ab\ncd\n");
    let set = SelectionSet::from_parts(vec![Selection::at(1), Selection::at(4)], 0);
    editor.current_mut().0.selections = set.unwrap();
    let target = editor.dictation_target().unwrap();
    // The person moves away while speaking; the text goes where they were.
    editor.current_mut().0.selections = SelectionSet::new(0);
    assert_eq!(
        editor.place_dictation(&target, "XY", false),
        DictationPlaced::Typed { span: None }
    );
    assert_eq!(text(&editor), "aXYb\ncXYd\n");
    undo(&mut editor);
    assert_eq!(text(&editor), "ab\ncd\n", "one step undoes both");
}

#[test]
fn a_place_that_changed_or_went_keeps_the_text_in_the_kill_ring() {
    let mut editor = editor("ab");
    let target = editor.dictation_target().unwrap();
    type_at(&mut editor, 2, "c");
    assert_eq!(
        editor.place_dictation(&target, "said", true),
        DictationPlaced::Kept("its text changed while you spoke")
    );
    assert_eq!(text(&editor), "abc");
    let newest = editor.kill_ring().get(0).map(|entry| {
        let pieces = entry.pieces().iter().map(crate::kill::Piece::text);
        pieces.collect::<String>()
    });
    assert_eq!(newest.as_deref(), Some("said"));

    let mut editor = super::tests::editor("ab");
    let target = editor.dictation_target().unwrap();
    let other = editor.add_buffer(Buffer::from_text("other.md", ""));
    editor.show(other);
    assert_eq!(
        editor.place_dictation(&target, "said", false),
        DictationPlaced::Kept("its window shows another buffer")
    );
}

#[test]
fn a_terminal_a_passphrase_a_question_and_a_read_only_buffer_are_refused() {
    let mut editor = Editor::default();
    let terminal = editor.add_terminal_buffer("*terminal*");
    editor.show(terminal);
    assert_eq!(
        editor.dictation_target(),
        Err("Dictation does not type into a terminal")
    );

    let mut editor = super::tests::editor("x");
    editor
        .open_prompt("Passphrase: ", Kind::Passphrase)
        .unwrap();
    assert_eq!(
        editor.dictation_target(),
        Err("Dictation never types a passphrase")
    );
    editor.quit();
    assert!(editor.dictation_target().is_ok());

    let mut editor = super::tests::editor("x");
    editor.ask("Go?", "keyboard-quit").unwrap();
    assert_eq!(
        editor.dictation_target(),
        Err("Dictation does not answer a question")
    );

    let mut editor = Editor::default();
    let internal = editor.add_internal_buffer("*Help*", crate::buffer::Mode::Fundamental);
    editor.show(internal);
    assert_eq!(
        editor.dictation_target(),
        Err("Dictation cannot type here: the buffer is read-only")
    );
}

#[test]
fn the_minibuffer_takes_the_text_while_its_prompt_is_open_with_no_span() {
    let mut editor = editor("x");
    editor
        .open_prompt("Find file: ", Kind::Text { history: None })
        .unwrap();
    let target = editor.dictation_target().unwrap();
    assert_eq!(
        editor.place_dictation(&target, "notes", true),
        DictationPlaced::Typed { span: None },
        "a prompt's answer is never fixed"
    );
    assert_eq!(text(&editor), "notes");
    editor.quit();
    assert_eq!(text(&editor), "x", "the buffer beneath is untouched");
    assert_eq!(
        editor.place_dictation(&target, "again", false),
        DictationPlaced::Kept("its window is gone")
    );
}

#[test]
fn the_text_waits_while_a_composition_is_open_in_its_window() {
    let mut editor = editor("x");
    let target = editor.dictation_target().unwrap();
    assert!(!editor.dictation_waits(&target));
    editor.set_preedit(Some(crate::display_map::Preedit::new("にほ".into(), None)));
    assert!(editor.dictation_waits(&target));
    editor.set_preedit(None);
    assert!(!editor.dictation_waits(&target));
}

#[test]
fn text_past_the_yanks_bound_across_the_cursors_goes_to_the_kill_ring() {
    let mut editor = editor("x");
    let mut target = editor.dictation_target().unwrap();
    target.cursors = vec![0; 300];
    let text = "a".repeat(64 * 1024);
    assert_eq!(
        editor.place_dictation(&target, &text, false),
        DictationPlaced::Kept("it is too much text for so many cursors")
    );
    assert_eq!(super::tests::text(&editor), "x");
    target.cursors = vec![0];
    assert!(matches!(
        editor.place_dictation(&target, &text, false),
        DictationPlaced::Typed { .. }
    ));
}

#[test]
fn a_secret_buffers_text_makes_a_secret_kill_and_no_span() {
    let mut editor = editor("x");
    let mut target = editor.dictation_target().unwrap();
    target.secret = true;
    editor.keep_dictation(&target, "pw", "its window is gone");
    assert!(editor.kill_ring().iter().next().unwrap().is_secret());

    let mut editor = secret_editor();
    let target = editor.dictation_target().unwrap();
    assert_eq!(
        editor.place_dictation(&target, "pw", true),
        DictationPlaced::Typed { span: None }
    );
}

#[test]
fn a_span_marks_the_text_at_each_cursor_and_follows_edits_outside_it() {
    let mut editor = editor("ab\ncd\n");
    let set = SelectionSet::from_parts(vec![Selection::at(1), Selection::at(4)], 0);
    editor.current_mut().0.selections = set.unwrap();
    let target = editor.dictation_target().unwrap();
    let span = span_of(editor.place_dictation(&target, "XY", true));
    assert_eq!(text(&editor), "aXYb\ncXYd\n");
    assert_eq!(ranges(&editor, span), [1..3, 6..8]);
    // Typed before it, at its start, and at its end: none of it is the dictation's.
    type_at(&mut editor, 0, "0");
    type_at(&mut editor, 2, "<");
    type_at(&mut editor, 5, ">");
    assert_eq!(text(&editor), "0a<XY>b\ncXYd\n");
    assert_eq!(ranges(&editor, span), [3..5, 9..11]);
    let context = editor.dictation_context(span, 3, 4096).unwrap();
    assert_eq!(context.text, "XY");
    assert_eq!(context.before, "0a<");
    assert_eq!(context.after, ">b\ncXYd\n");
}

#[test]
fn a_fix_replaces_inside_the_span_in_one_undo_step_and_ends_it() {
    let mut editor = editor("前\n");
    let target = editor.dictation_target().unwrap();
    let span = span_of(editor.place_dictation(&target, "りあくとを使う", true));
    let sent = editor.dictation_context(span, 1, 64).unwrap().text;
    assert_eq!(sent, "りあくとを使う");
    let edits = [(0.."りあくと".len(), "React")];
    assert_eq!(editor.fix_dictation(span, &sent, &edits), Ok(()));
    assert_eq!(text(&editor), "Reactを使う前\n");
    assert!(editor.dictation_span_at(span).is_none(), "the span ended");
    undo(&mut editor);
    assert_eq!(
        text(&editor),
        "りあくとを使う前\n",
        "one undo brings back the raw text"
    );
    undo(&mut editor);
    assert_eq!(text(&editor), "前\n");
    assert_eq!(
        editor.fix_dictation(span, &sent, &edits),
        Err("its text is gone")
    );
}

#[test]
fn a_fix_of_an_edited_span_is_skipped_and_changes_nothing() {
    let mut editor = editor("");
    let target = editor.dictation_target().unwrap();
    let span = span_of(editor.place_dictation(&target, "abcd", true));
    type_at(&mut editor, 2, "!");
    assert_eq!(
        editor.fix_dictation(span, "abcd", &[(0..1, "A")]),
        Err("its text was edited")
    );
    assert_eq!(text(&editor), "ab!cd");
    assert!(editor.dictation_span_at(span).is_none());
}

#[test]
fn a_fix_that_does_not_fit_the_text_sent_changes_nothing() {
    #[expect(
        clippy::reversed_empty_ranges,
        reason = "a range that ends before it starts is what is refused"
    )]
    let reversed = 2..1;
    for edits in [
        vec![(reversed, "x")],
        vec![(0..9, "x")],
        vec![(2..3, "x"), (0..1, "y")],
        vec![(1..2, "x")],
    ] {
        let mut editor = editor("");
        let target = editor.dictation_target().unwrap();
        let span = span_of(editor.place_dictation(&target, "あい", true));
        assert_eq!(
            editor.fix_dictation(span, "あい", &edits),
            Err("the fix did not fit the text"),
            "{edits:?}"
        );
        assert_eq!(text(&editor), "あい");
    }
}

#[test]
fn a_span_goes_with_its_buffer_a_reload_and_past_four() {
    let mut editor = editor("");
    let spans: Vec<DictationPlaced> = (0..5)
        .map(|_| {
            let target = editor.dictation_target().unwrap();
            editor.place_dictation(&target, "a", true)
        })
        .collect();
    assert!(
        spans[..4]
            .iter()
            .all(|placed| matches!(placed, DictationPlaced::Typed { span: Some(_) }))
    );
    assert_eq!(spans[4], DictationPlaced::Typed { span: None });
    let first = span_of(spans[0]);
    editor.end_dictation_span(first);
    assert!(editor.dictation_span_at(first).is_none());
    let span = span_of(editor.place_dictation(&editor.dictation_target().unwrap(), "b", true));
    let id = editor.window().buffer();
    editor.forget_dictation_spans(id);
    assert!(editor.dictation_context(span, 3, 64).is_none());
}

#[test]
fn a_span_is_drawn_in_its_window_and_never_in_a_secret_buffer() {
    let mut editor = editor("ab\n");
    editor.current_mut().0.selections = SelectionSet::new(1);
    let target = editor.dictation_target().unwrap();
    let span = span_of(editor.place_dictation(&target, "XYZ", true));
    let mut cells = Vec::new();
    let id = editor.tiled().0.id();
    editor.dictation_span_cells(id, &editor.screen_now(), &mut cells);
    assert_eq!(cells.len(), 1);
    assert_eq!((cells[0].row, cells[0].column, cells[0].columns), (0, 1, 3));
    editor.end_dictation_span(span);
    editor.dictation_span_cells(id, &editor.screen_now(), &mut cells);
    assert!(cells.is_empty(), "the face goes with the span");

    let editor = secret_editor();
    assert!(editor.dictation_context(DictationSpan(0), 3, 64).is_none());
}

#[test]
fn the_context_is_cut_to_its_lines_and_bytes_at_characters() {
    let mut editor = editor("1\n2\n3\n4\nあいう\n6\n7\n8\n9\n");
    editor.current_mut().0.selections = SelectionSet::new("1\n2\n3\n4\nあ".len());
    let target = editor.dictation_target().unwrap();
    let span = span_of(editor.place_dictation(&target, "X", true));
    let context = editor.dictation_context(span, 2, 4096).unwrap();
    assert_eq!(context.before, "3\n4\nあ");
    assert_eq!(context.after, "いう\n6\n7");
    assert_eq!(context.mode, editor.current().1.mode().name());
    let context = editor.dictation_context(span, 2, 4).unwrap();
    assert_eq!(context.before, "\nあ", "cut at a character's start");
    assert_eq!(context.after, "い", "cut at a character's end");
    let context = editor.dictation_context(span, 0, 4096).unwrap();
    assert_eq!(context.before, "あ");
    assert_eq!(context.after, "いう");
}

#[test]
fn a_fix_is_bounded_by_its_cursors_and_its_replacements() {
    let lines = "\n".repeat(MAX_SPAN_RANGES + 1);
    let at_each = |editor: &mut Editor, count: usize| {
        let cursors = (0..count).map(Selection::at).collect();
        editor.current_mut().0.selections = SelectionSet::from_parts(cursors, 0).unwrap();
    };
    // Past the cursors a span takes, the text is typed plain.
    let mut editor = editor(&lines);
    at_each(&mut editor, MAX_SPAN_RANGES + 1);
    let target = editor.dictation_target().unwrap();
    assert_eq!(
        editor.place_dictation(&target, "ab", true),
        DictationPlaced::Typed { span: None }
    );

    let mut editor = super::tests::editor(&lines);
    at_each(&mut editor, MAX_SPAN_RANGES);
    let target = editor.dictation_target().unwrap();
    let sent = "a".repeat(64);
    let span = span_of(editor.place_dictation(&target, &sent, true));
    let edits: Vec<(Range<usize>, &str)> = (0..64).map(|at| (at..at + 1, "b")).collect();
    assert!(edits.len() * MAX_SPAN_RANGES > MAX_FIX_REPLACEMENTS);
    assert_eq!(
        editor.fix_dictation(span, &sent, &edits),
        Err("it would change too many places at once")
    );
    assert!(!text(&editor).contains('b'));
}

#[test]
fn an_undo_of_the_text_leaves_no_fix_with_or_without_an_edit_log() {
    // A Markdown buffer keeps a log of its edits, and a plain one does not.
    for name in ["notes.md", "notes"] {
        let mut editor = Editor::with_buffer(Buffer::from_text(name, "x"));
        editor.set_view_size(ViewSize::new(10, 40));
        let target = editor.dictation_target().unwrap();
        let span = span_of(editor.place_dictation(&target, "said", true));
        undo(&mut editor);
        assert_eq!(text(&editor), "x", "{name}");
        assert!(
            editor.fix_dictation(span, "said", &[(0..1, "S")]).is_err(),
            "{name}: the span no longer holds the text"
        );
        assert_eq!(text(&editor), "x", "{name}");
    }
}

#[test]
fn a_buffer_that_turns_secret_gives_no_context_and_takes_no_fix() {
    let mut editor = editor("");
    let target = editor.dictation_target().unwrap();
    let span = span_of(editor.place_dictation(&target, "said", true));
    assert!(editor.dictation_context(span, 3, 64).is_some());
    let id = editor.window().buffer();
    editor.buffer_mut(id).unwrap().judge_copy(
        &[std::path::PathBuf::from("/tmp/.env")],
        crate::secret::Verdict::SecretName,
    );
    assert!(editor.dictation_context(span, 3, 64).is_none());
    let mut cells = Vec::new();
    editor.dictation_span_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
    assert!(cells.is_empty(), "no face over a secret buffer");
    assert_eq!(
        editor.fix_dictation(span, "said", &[(0..1, "S")]),
        Err("its buffer may hold secrets")
    );
    assert_eq!(text(&editor), "said");
}
