//! Tests of windows' wraps through [`Editor`] (ADR 0100, ADR 0103): turning wrapping on and
//! off, the top as an offset through edits, places and reloads, and jobs and their reports.
//!
//! Must not depend on: `command` or `keymap`, or threads; the jobs run where the test takes
//! them, and the budgets are the small ones of tests, so that rows are checked once settled.

use super::edit_tests::{edit, editor_with};
use super::*;
use crate::display_map::wrap_map::WrapSnapshot;
use crate::window::{Scroll, ViewSize};

/// Runs every job the windows want until none is left.
pub(super) fn settle(editor: &mut Editor) {
    for _ in 0..100 {
        let jobs = editor.take_wrap_jobs();
        if jobs.is_empty() {
            return;
        }
        for job in jobs {
            if let Some(report) = job.run() {
                editor.take_wrap_report(report);
            }
        }
    }
    panic!("the jobs never settled");
}

/// Each view row's start and whether it continues, as the selected window has them.
pub(super) fn rows(editor: &Editor) -> Vec<(usize, bool)> {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let wrap = window.wrap().expect("wrapped");
    shape(wrap, &display)
}

fn shape(wrap: &WrapSnapshot, display: &crate::display_map::DisplaySnapshot) -> Vec<(usize, bool)> {
    (0..wrap.row_count())
        .map(|row| {
            let row = wrap.row(display, row);
            (row.start, row.continues)
        })
        .collect()
}

/// The rows of a wrap from scratch of the selected window's buffer at its width.
pub(super) fn fresh(editor: &Editor) -> Vec<(usize, bool)> {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    shape(
        &WrapSnapshot::new(&display, window.size().columns),
        &display,
    )
}

fn wrapped(text: &str, size: ViewSize) -> (Editor, BufferId) {
    let (mut editor, id) = editor_with(text);
    editor.set_view_size(size);
    assert!(editor.set_wraps(id, true));
    (editor, id)
}

#[test]
fn turning_wrapping_on_gives_the_window_view_rows_at_its_width() {
    let text = format!("{}\nend\n", "word ".repeat(100));
    let (mut editor, id) = wrapped(&text, ViewSize::new(10, 20));
    assert!(editor.buffer(id).unwrap().wraps());
    assert!(editor.window().wrap().is_some());
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
    assert!(rows(&editor).len() > 20, "the long line wraps");
    assert!(!editor.set_wraps(id, true), "already on");
}

#[test]
fn turning_wrapping_off_drops_the_wrap_and_its_jobs_report() {
    let text = "word ".repeat(100);
    let (mut editor, id) = wrapped(&text, ViewSize::new(10, 20));
    let jobs = editor.take_wrap_jobs();
    assert_eq!(jobs.len(), 1);
    assert!(editor.set_wraps(id, false));
    assert!(editor.window().wrap().is_none());
    assert_eq!(editor.window().row_count(&editor.current().1.display()), 1);
    for job in jobs {
        assert!(job.run().is_none(), "its map is gone, so it stops");
    }
}

#[test]
fn a_report_of_a_wrap_turned_off_and_on_again_is_not_taken() {
    let text = "word ".repeat(100);
    let (mut editor, id) = wrapped(&text, ViewSize::new(10, 20));
    let job = editor.take_wrap_jobs().pop().expect("a job");
    // A clone of the map shares its generation, and keeps it alive, so the job finishes.
    let kept = editor.window().wrap_map().cloned();
    editor.set_wraps(id, false);
    editor.set_wraps(id, true);
    let report = job.run().expect("a report");
    assert!(!editor.take_wrap_report(report), "another map's");
    drop(kept);
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
}

#[test]
fn edits_keep_the_rows_in_step_and_a_report_takes_the_edits_since_its_job() {
    let text = format!(
        "{}\n{}",
        "word ".repeat(40),
        "日本語の文章です。".repeat(10)
    );
    let (mut editor, id) = wrapped(&text, ViewSize::new(10, 16));
    let jobs = editor.take_wrap_jobs();
    editor
        .apply_edits(id, &[edit(3..3, "longer "), edit(0..0, "x")])
        .unwrap();
    editor.apply_edits(id, &[edit(250..250, "あい")]).unwrap();
    for job in jobs {
        if let Some(report) = job.run() {
            editor.take_wrap_report(report);
        }
    }
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
    editor.undo().unwrap();
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor), "undo too");
}

#[test]
fn an_edit_above_the_view_keeps_the_text_at_its_top() {
    let text: String = (0..100).map(|i| format!("line {i:03}\n")).collect();
    let (mut editor, id) = editor_with(&text);
    editor.set_view_size(ViewSize::new(10, 20));
    let top = text.find("line 050").unwrap();
    let cursor = text.find("line 055").unwrap();
    editor.restore_positions(
        SelectionSet::new(cursor),
        Scroll {
            start: top,
            left: 0,
        },
    );
    assert_eq!(editor.top_row(), 50);
    editor.apply_edits(id, &[edit(0..0, "new\n")]).unwrap();
    assert_eq!(
        editor.window().scroll().start,
        top + 4,
        "it moved with its text"
    );
    assert_eq!(editor.top_row(), 51);
    editor
        .apply_edits(id, &[edit(top + 4..top + 4, "at it ")])
        .unwrap();
    assert_eq!(
        editor.window().scroll().start,
        top + 4,
        "text inserted at the top shows there"
    );
    editor
        .apply_edits(id, &[edit(top + 1..top + 9, "")])
        .unwrap();
    assert_eq!(
        editor.window().scroll().start,
        top + 1,
        "a deletion over the top takes it to where the deletion was"
    );
    assert_eq!(
        editor.top_row(),
        50,
        "and the view shows from that row's start"
    );
}

#[test]
fn a_place_keeps_the_top_and_the_wrap_comes_back_at_return() {
    let text = format!("{}\n{}", "word ".repeat(80), "more ".repeat(80));
    let (mut editor, a) = wrapped(&text, ViewSize::new(5, 20));
    settle(&mut editor);
    let cursor = text.len() - 3;
    let fresh_rows = fresh(&editor);
    let start = fresh_rows[20].0;
    editor.restore_positions(SelectionSet::new(cursor), Scroll { start, left: 0 });
    let kept = editor.window().scroll();
    let b = editor.add_buffer(Buffer::from_text("b", "other\n"));
    editor.show(b);
    assert!(editor.window().wrap().is_none(), "b does not wrap");
    editor.show(a);
    assert!(editor.window().wrap().is_some(), "made again");
    assert_eq!(editor.window().scroll(), kept);
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
    assert_eq!(
        editor.window().scroll(),
        kept,
        "a real row's start stays one"
    );
}

#[test]
fn a_load_that_changes_the_text_behind_the_wrap_starts_it_over() {
    let text = "word ".repeat(60);
    let (mut editor, id) = wrapped(&text, ViewSize::new(5, 20));
    let more = "word ".repeat(90);
    let snapshot =
        crate::load::decode_all(more.as_bytes(), crate::display_map::TabWidth::default());
    editor.buffer_mut(id).unwrap().apply_load(snapshot);
    editor.loaded(id);
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
}

#[test]
fn a_window_too_narrow_to_wrap_shows_a_row_a_line() {
    let text = format!("{}\nend\n", "word ".repeat(30));
    let (mut editor, _) = wrapped(&text, ViewSize::new(5, 7));
    settle(&mut editor);
    assert_eq!(rows(&editor).len(), 3);
    assert!(editor.take_wrap_jobs().is_empty());
    editor.set_view_size(ViewSize::new(5, 20));
    settle(&mut editor);
    assert_eq!(
        rows(&editor),
        fresh(&editor),
        "and wraps once it is wide enough"
    );
}

#[test]
fn nothing_scrolls_sideways_while_wrapped() {
    let text = "word ".repeat(30);
    let (mut editor, _) = wrapped(&text, ViewSize::new(5, 20));
    assert!(!editor.scroll_view_sideways(10));
    assert_eq!(editor.window().scroll().left, 0);
}

#[test]
fn many_cursors_typing_and_undoing_keep_the_rows_in_step() {
    use crate::selection::Selection;
    let text = format!(
        "{}\n{}",
        "word and more ".repeat(60),
        "日本語の文章です。".repeat(20)
    );
    let (mut editor, id) = wrapped(&text, ViewSize::new(10, 16));
    // More splices to a command than a rewrap reads the snapshots of.
    let places: Vec<usize> = (0..40).map(|i| i * 20).collect();
    let selections = places.iter().copied().map(Selection::at).collect();
    editor.current_mut().0.selections = SelectionSet::from_parts(selections, 0).unwrap();
    for _ in 0..3 {
        editor.begin_undo_group();
        editor.allow_undo_join();
        let edits: Vec<_> = (editor.window().selections.iter())
            .map(|selection| edit(selection.cursor..selection.cursor, "xy"))
            .collect();
        editor.apply_edits(id, &edits).unwrap();
        editor.end_undo_group();
    }
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
    editor.begin_undo_group();
    editor.undo().unwrap();
    editor.end_undo_group();
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor), "and undoing them as one");
}

#[test]
fn messages_logged_to_a_wrapped_messages_buffer_keep_its_rows_in_step() {
    let (mut editor, _) = editor_with("text");
    editor.set_view_size(ViewSize::new(5, 20));
    editor.toggle_messages();
    let messages = editor.window().buffer();
    assert!(editor.set_wraps(messages, true));
    for count in 0..5 {
        editor.message(format!(
            "message {count} {}",
            "said at length ".repeat(count)
        ));
    }
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
}

#[test]
fn a_whole_text_put_back_into_a_wrapped_buffer_starts_its_wrap_over() {
    let (mut editor, id) = wrapped(&"word ".repeat(60), ViewSize::new(5, 20));
    settle(&mut editor);
    let copy = editor.add_buffer(Buffer::from_text("copy", &"other words ".repeat(50)));
    editor.replace_text_from(id, copy).unwrap();
    let jobs = editor.take_wrap_jobs();
    assert_eq!(jobs.len(), 1, "a job for the new text");
    for job in jobs {
        let report = job.run().expect("a report");
        assert!(editor.take_wrap_report(report));
    }
    settle(&mut editor);
    assert_eq!(rows(&editor), fresh(&editor));
}
