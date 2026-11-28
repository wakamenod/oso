//! Tests of the commands of paragraphs, sentences and pages: several cursors, counts and
//! negative ones, Shift, the region they grow, repeated marks, Markdown, kills that append,
//! and a count past the budget (ADR 0190).
//!
//! Must not depend on: winit, the GPU or `bindings`.

use oso_core::command::{Args, Ctx, Effects, Invocation};
use oso_core::editor::Editor;
use oso_core::selection::{MarkActive, Selection, SelectionSet};

use crate::commands::registry;
use crate::commands::tests::{cursor, editor, run};

/// Runs `name` with `args`, as the `repeats`-th press in a row, with Shift if `shifted`.
fn press(editor: &mut Editor, name: &str, args: Args, repeats: usize, shifted: bool) {
    let registry = registry();
    let id = registry.lookup(name).unwrap();
    let invocation = Invocation {
        repeats,
        shifted,
        ..Invocation::default()
    };
    let mut ctx = Ctx::invoked(editor, invocation);
    registry.execute(id, &mut ctx, args).unwrap();
}

fn cursors(editor: &Editor) -> Vec<usize> {
    editor
        .window()
        .selections
        .iter()
        .map(|s| s.cursor)
        .collect()
}

/// The regions of the selected window, cursor first.
fn regions(editor: &Editor) -> Vec<(usize, Option<usize>)> {
    let selections = editor.window().selections.iter();
    selections.map(|s| (s.cursor, s.mark)).collect()
}

fn set_cursors(editor: &mut Editor, offsets: &[usize]) {
    let selections = offsets.iter().map(|&at| Selection::at(at)).collect();
    editor.current_mut().0.selections = SelectionSet::from_parts(selections, 0).unwrap();
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

#[test]
fn paragraphs_move_every_cursor_with_counts_both_ways() {
    let mut editor = editor("a\n\nb\n\nc\n\nd");
    set_cursors(&mut editor, &[0, 3]);
    assert!(run(&mut editor, "forward-paragraph", Args::NONE).1.redraw);
    assert_eq!(cursors(&editor), [2, 5]);
    let _ = run(&mut editor, "forward-paragraph", Args::numeric(2));
    assert_eq!(cursors(&editor), [8, 10]);
    let _ = run(&mut editor, "backward-paragraph", Args::numeric(-1));
    assert_eq!(cursors(&editor), [10], "the cursors met at the end");
    let _ = run(&mut editor, "backward-paragraph", Args::numeric(2));
    assert_eq!(cursors(&editor), [5]);
    let _ = run(&mut editor, "forward-paragraph", Args::numeric(-9));
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn the_buffer_edges_stop_a_motion_with_no_signal() {
    let mut editor = editor("a.\n\nb.");
    for name in ["backward-paragraph", "backward-sentence", "backward-page"] {
        assert_eq!(
            run(&mut editor, name, Args::NONE),
            (Ok(()), Effects::default()),
            "{name}"
        );
    }
    let _ = run(&mut editor, "end-of-buffer", Args::NONE);
    for name in ["forward-paragraph", "forward-sentence", "forward-page"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert_eq!(result, Ok(()), "{name}");
        assert_eq!(cursor(&editor), 6);
    }
}

#[test]
fn markdown_headings_are_paragraphs_of_their_own() {
    let text = "# Title\ntext\n```\ncode\n```";
    let mut fundamental = editor(text);
    let _ = run(&mut fundamental, "forward-paragraph", Args::NONE);
    assert_eq!(cursor(&fundamental), text.len());
    let mut markdown = editor(text);
    let _ = run(&mut markdown, "markdown-mode", Args::NONE);
    let _ = run(&mut markdown, "forward-paragraph", Args::NONE);
    assert_eq!(cursor(&markdown), 8);
    let _ = run(&mut markdown, "forward-paragraph", Args::NONE);
    assert_eq!(cursor(&markdown), 13);
}

#[test]
fn sentences_move_every_cursor() {
    let mut editor = editor("One. Two. Three.\n文一。文二。");
    let wide = "One. Two. Three.\n".len();
    set_cursors(&mut editor, &[0, wide]);
    let _ = run(&mut editor, "forward-sentence", Args::NONE);
    assert_eq!(cursors(&editor), [4, wide + "文一。".len()]);
    let _ = run(&mut editor, "forward-sentence", Args::numeric(2));
    assert_eq!(cursors(&editor), [16, wide + "文一。文二。".len()]);
    let _ = run(&mut editor, "backward-sentence", Args::NONE);
    assert_eq!(cursors(&editor), [10, wide + "文一。".len()]);
}

#[test]
fn a_motion_grows_a_shown_region_and_selects_with_shift() {
    let mut editor = editor("One. Two. Three.");
    let _ = run(&mut editor, "set-mark-command", Args::NONE);
    let _ = run(&mut editor, "forward-sentence", Args::NONE);
    assert_eq!(regions(&editor), [(4, Some(0))]);
    assert!(editor.window().selections.mark_active().is_active());

    let mut editor = crate::commands::tests::editor("One. Two. Three.");
    press(&mut editor, "forward-sentence", Args::NONE, 0, true);
    assert_eq!(regions(&editor), [(4, Some(0))]);
    assert_eq!(
        editor.window().selections.mark_active(),
        MarkActive::Shifted
    );
    press(&mut editor, "forward-sentence", Args::NONE, 0, false);
    assert_eq!(cursor(&editor), 9);
    assert!(!editor.window().selections.mark_active().is_active());
}

#[test]
fn mark_paragraph_marks_it_and_repeated_takes_more() {
    let mut editor = editor("a\n\nb\nc\n\nd\n\ne");
    let _ = run(&mut editor, "forward-char", Args::numeric(3));
    let _ = run(&mut editor, "mark-paragraph", Args::NONE);
    assert_eq!(regions(&editor), [(2, Some(7))]);
    assert_eq!(editor.window().selections.mark_active(), MarkActive::Active);
    press(&mut editor, "mark-paragraph", Args::NONE, 1, false);
    assert_eq!(regions(&editor), [(2, Some(10))]);
    // While the region shows, it goes on too, by N.
    press(&mut editor, "mark-paragraph", Args::numeric(-1), 0, false);
    assert_eq!(regions(&editor), [(2, Some(7))]);
    // There was no mark before, so none went to the ring.
    assert_eq!(editor.window().selections.mark_ring().iter().len(), 0);
    assert_eq!(
        run(&mut editor, "mark-paragraph", Args::numeric(0)).0,
        Err("Cannot mark zero paragraphs".into())
    );
}

#[test]
fn mark_paragraph_with_a_count_and_at_several_cursors() {
    let mut editor = editor("a\n\nb\n\nc\n\nd");
    let _ = run(&mut editor, "mark-paragraph", Args::numeric(2));
    assert_eq!(regions(&editor), [(0, Some(5))]);
    // A new set of cursors, with no region and an empty ring.
    set_cursors(&mut editor, &[0, 6]);
    let _ = run(&mut editor, "set-mark-command", Args::NONE);
    press(&mut editor, "set-mark-command", Args::NONE, 1, false);
    assert!(!editor.window().selections.mark_active().is_active());
    let _ = run(&mut editor, "mark-paragraph", Args::NONE);
    assert_eq!(regions(&editor), [(0, Some(2)), (5, Some(8))]);
    // The primary's old mark went first in the ring.
    let ring: Vec<usize> = editor.window().selections.mark_ring().iter().collect();
    assert_eq!(ring, [0]);
}

#[test]
fn mark_page_marks_this_page_or_one_n_pages_away() {
    let text = "a\n\u{c}b\n\u{c}c\n\u{c}d";
    let mut editor = editor(text);
    let _ = run(&mut editor, "forward-char", Args::numeric(4));
    let _ = run(&mut editor, "mark-page", Args::NONE);
    assert_eq!(regions(&editor), [(3, Some(6))]);
    press(&mut editor, "mark-page", Args::NONE, 1, false);
    assert_eq!(regions(&editor), [(3, Some(9))]);
    let mut editor = crate::commands::tests::editor(text);
    let _ = run(&mut editor, "mark-page", Args::numeric(1));
    assert_eq!(regions(&editor), [(3, Some(6))]);
    let mut editor = crate::commands::tests::editor(text);
    let _ = run(&mut editor, "end-of-buffer", Args::NONE);
    let _ = run(&mut editor, "set-mark-command", Args::NONE);
    press(&mut editor, "set-mark-command", Args::NONE, 1, false);
    let _ = run(&mut editor, "mark-page", Args::numeric(-1));
    assert_eq!(regions(&editor), [(6, Some(9))], "the page before this one");
}

#[test]
fn pages_move_every_cursor() {
    let mut editor = editor("a\n\u{c}b\n\u{c}c");
    set_cursors(&mut editor, &[0, 4]);
    let _ = run(&mut editor, "forward-page", Args::NONE);
    assert_eq!(cursors(&editor), [3, 6]);
    let _ = run(&mut editor, "backward-page", Args::NONE);
    assert_eq!(cursors(&editor), [0, 3]);
}

#[test]
fn kill_sentence_kills_at_every_cursor_and_appends() {
    let mut editor = editor("One. Two. Three.\nA. B. C.");
    set_cursors(&mut editor, &[0, 17]);
    let _ = run(&mut editor, "kill-sentence", Args::NONE);
    assert_eq!(text(&editor), " Two. Three.\n B. C.");
    editor.note_command_ended(false);
    let _ = run(&mut editor, "kill-sentence", Args::NONE);
    assert_eq!(text(&editor), " Three.\n C.");
    let ring = editor.kill_ring();
    assert_eq!(ring.len(), 1, "the second kill appended");
    let pieces: Vec<String> = ring
        .get(0)
        .unwrap()
        .pieces()
        .iter()
        .map(|p| p.text())
        .collect();
    assert_eq!(pieces, ["One. Two.", "A. B."]);
}

#[test]
fn backward_kill_sentence_kills_back_to_the_start() {
    let mut editor = editor("One. Two three. Four.");
    let _ = run(&mut editor, "forward-char", Args::numeric(9));
    let _ = run(&mut editor, "backward-kill-sentence", Args::NONE);
    assert_eq!(text(&editor), "One. three. Four.");
    editor.note_command_ended(false);
    let _ = run(&mut editor, "backward-kill-sentence", Args::NONE);
    assert_eq!(text(&editor), "three. Four.");
    let pieces: Vec<String> = editor
        .kill_ring()
        .get(0)
        .unwrap()
        .pieces()
        .iter()
        .map(|p| p.text())
        .collect();
    assert_eq!(pieces, ["One. Two "], "the text before went first");
    let _ = run(&mut editor, "kill-sentence", Args::numeric(-1));
    assert_eq!(text(&editor), "three. Four.", "nothing before");
}

#[test]
fn a_count_past_the_budget_signals_and_kills_nothing() {
    let text = format!("{}.", "word ".repeat(1 << 20));
    let mut editor = editor(&text);
    let (result, _) = run(&mut editor, "kill-sentence", Args::NONE);
    assert_eq!(result, Err("Count too large".into()));
    assert_eq!(editor.current().1.text().len(), text.len());
    let (result, _) = run(&mut editor, "forward-sentence", Args::NONE);
    assert_eq!(result, Err("Count too large".into()));
    assert_eq!(cursor(&editor), 0);
}
