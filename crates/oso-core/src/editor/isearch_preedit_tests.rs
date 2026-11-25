//! Tests of what an input method composes in isearch (ADR 0158): where it shows, what it leaves
//! alone in the buffer, and that it is withheld around one that may hold secrets.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::test_buffers::secret_buffer;
use super::{Editor, Status};
use crate::buffer::Buffer;
use crate::display_map::Preedit;
use crate::frame::{CellRect, PixelGrid};
use crate::window::{Screen, ViewSize};

/// An editor whose searches are literal: migemo's are in `isearch_migemo_tests`.
fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_view_size(ViewSize::new(10, 80));
    editor.set_migemo(false);
    editor
}

/// Runs the worker's job in place, as the host's thread would, and takes in its reports.
fn drain(editor: &mut Editor) {
    while let Some(job) = editor.take_isearch_job() {
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        for report in reports {
            editor.isearch_reported(report);
        }
    }
}

fn line(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

fn cursor(editor: &Editor) -> usize {
    editor.window().selections.primary().cursor
}

fn search(editor: &mut Editor, text: &str) {
    editor.isearch_begin(false, false);
    editor.isearch_add(text);
    drain(editor);
}

/// What an input method composes shows on the search's line, after the string, and nowhere in
/// the buffer, whose view and match stay as they are until the commit (ADR 0158).
#[test]
fn a_composition_shows_on_the_searchs_line_and_not_in_the_buffer() {
    let mut editor = editor("か かな かな\n");
    search(&mut editor, "か");
    let (at, before) = (cursor(&editor), line(&editor));
    assert_eq!(before, "I-search: か [1/3]");
    let composed = |editor: &Editor| {
        let mut status = Status::default();
        editor.status_into(editor.tiled().0.id(), &mut status);
        status.composed.map(|range| status.echo[range].to_owned())
    };
    assert_eq!(composed(&editor).as_deref(), Some(""));
    assert!(editor.excerpt_stamp().is_none(), "nothing of the buffer");
    // The candidates open where the string goes on, on the echo row.
    let grid = PixelGrid::new(10.0, 20.0);
    let echo = editor.frame().echo;
    let cell = grid.rect(CellRect {
        column: 12,
        rows: 1,
        columns: 1,
        ..echo
    });
    assert_eq!(editor.cursor_area(grid), Some(cell));

    let mut plain = Screen::default();
    editor.screen(&mut plain);
    editor.set_preedit(Some(Preedit::new("な".into(), None)));
    assert_eq!(line(&editor), "I-search: かな [1/3]");
    assert_eq!(composed(&editor).as_deref(), Some("な"));
    assert_eq!(cursor(&editor), at);
    let mut screen = Screen::default();
    editor.screen(&mut screen);
    assert_eq!(screen.preedit(), None, "the buffer's text is not pushed");
    let mut positions = Vec::new();
    assert_eq!(editor.preedit_layout(grid, &mut positions), Some(cell));
    assert_eq!(positions, [0.0, 20.0]);
    // A message does not hide it.
    editor.echo("Saved");
    assert_eq!(composed(&editor).as_deref(), Some("な"));
    editor.clear_echo();

    editor.set_preedit(None);
    editor.isearch_add("な");
    drain(&mut editor);
    assert_eq!(line(&editor), "I-search: かな [1/2]");
    editor.isearch_delete();
    drain(&mut editor);
    assert_eq!(line(&editor), before, "one step back for the commit");
    editor.isearch_exit();
    assert_eq!(composed(&editor), None);
    assert!(editor.excerpt_stamp().is_some());
}

#[test]
fn a_composition_after_a_long_string_stays_in_view() {
    let mut editor = editor("x");
    search(&mut editor, &"x".repeat(100));
    editor.set_preedit(Some(Preedit::new("かな".into(), None)));
    let shown = line(&editor);
    let end = shown.find("かな").unwrap();
    assert!(shown.starts_with("…x"), "{shown}");
    assert_eq!(shown[..end].chars().count(), 76, "with four cells, the row");
    // One wider than the row starts at its left.
    editor.set_preedit(Some(Preedit::new("か".repeat(50), None)));
    assert!(line(&editor).starts_with("かか"));
}

#[test]
fn a_composition_in_a_search_around_a_secret_is_withheld_with_it() {
    let mut editor = Editor::with_buffer(secret_buffer());
    editor.set_view_size(ViewSize::new(10, 80));
    editor.isearch_begin(false, false);
    editor.set_preedit(Some(Preedit::new("か".into(), None)));
    assert!(editor.preedit_withheld() && editor.around_secret());
    assert!(editor.isearch().unwrap().around_secret());
}
