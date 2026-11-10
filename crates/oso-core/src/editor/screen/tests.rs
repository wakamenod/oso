//! Tests of the rows a frame shows through [`Editor`]: the tiled window's with a preedit,
//! following a preedit that wraps, and the preview's wrap (ADR 0104).
//!
//! Must not depend on: `command` or `keymap`, or threads.

use super::super::edit_tests::plain_edit;
use super::*;
use crate::buffer::{Buffer, BufferId};
use crate::complete::Candidate;
use crate::display_map::Preedit;
use crate::editor::CompletionStyle;
use crate::frame::FrameSize;
use crate::selection::SelectionSet;
use crate::window::ViewSize;

/// Each row as (line, column, whether it continues).
fn shape(screen: &Screen) -> Vec<(usize, usize, bool)> {
    screen
        .rows()
        .iter()
        .map(|row| (row.line, row.column, row.continues))
        .collect()
}

/// An editor whose `*scratch*` window shows a buffer `a` of `text` while `b` is shown, with
/// `C-x b`'s prompt open on `a` in a frame of 30 rows by 100 columns.
fn previewing(text: &str, wraps: bool) -> (Editor, BufferId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 100,
    });
    let a = editor.add_buffer(Buffer::from_text("a", text));
    editor.set_wraps(a, wraps);
    let names = ["a"].map(|name| Candidate::new(name, "", ""));
    let style = CompletionStyle {
        given_order: true,
        takes_input: true,
        preview: true,
    };
    editor
        .open_completion_as("Switch to buffer:", "buffer", names.into(), style)
        .unwrap();
    editor.follow_cursor();
    (editor, a)
}

#[test]
fn the_tiled_windows_rows_lay_a_preedit_out_in_its_line() {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", "aaaa bbb\nnext"));
    editor.set_view_size(ViewSize::new(10, 9));
    let id = editor.window().buffer();
    editor.set_wraps(id, true);
    editor.current_mut().0.selections = SelectionSet::new(8);
    editor.follow_cursor();
    let mut screen = Screen::default();
    editor.screen(&mut screen);
    assert_eq!(shape(&screen), [(0, 0, false), (1, 0, false)]);
    editor.set_preedit(Some(Preedit::new("xyz".into(), None)));
    editor.screen(&mut screen);
    assert_eq!(shape(&screen), [(0, 0, true), (0, 5, false), (1, 0, false)]);
}

#[test]
fn a_preview_wraps_the_buffer_at_its_own_width() {
    let (mut editor, _) = previewing(&"word ".repeat(400), true);
    let region = editor.prompt_layout().unwrap().regions.preview;
    assert!(region.rows > 2 && region.columns >= 8);
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    assert!(
        screen.rows()[0].continues,
        "the first row is wrapped at once"
    );
    // The budgets of tests are small: each command wraps a little more near the start.
    for _ in 0..100 {
        editor.follow_cursor();
    }
    editor.preview_screen(&mut screen);
    assert_eq!(screen.rows().len(), region.rows);
    let rows = screen.rows();
    assert!(rows.iter().all(|row| row.line == 0 && row.continues));
    // Rows of whole words in the width, the mark's column aside.
    assert!(rows.windows(2).all(|pair| {
        let width = pair[1].column - pair[0].column;
        width <= region.columns && width % 5 == 0
    }));
}

#[test]
fn a_preview_of_a_buffer_that_does_not_wrap_shows_its_lines() {
    let (editor, _) = previewing(&format!("{}\nend", "word ".repeat(100)), false);
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    assert_eq!(shape(&screen), [(0, 0, false), (1, 0, false)]);
}

#[test]
fn a_preview_follows_its_buffers_text_and_goes_with_the_prompt() {
    let (mut editor, a) = previewing("short", true);
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    assert_eq!(shape(&screen), [(0, 0, false)]);
    plain_edit(&mut editor, a, 5..5, &" word".repeat(100));
    editor.follow_cursor();
    editor.preview_screen(&mut screen);
    assert!(screen.rows().len() > 1, "wrapped anew");
    editor.quit();
    editor.follow_cursor();
    editor.preview_screen(&mut screen);
    assert!(screen.rows().is_empty());
    assert!(editor.preview_wrap.is_none());
}

#[test]
fn the_view_follows_a_preedit_that_wraps_past_the_last_row() {
    // Three rows of 9 columns, the cursor at the end of the last one in view.
    let mut editor = Editor::with_buffer(Buffer::from_text("t", "a\nb\naaaa bbb"));
    editor.set_view_size(ViewSize::new(3, 9));
    let id = editor.window().buffer();
    editor.set_wraps(id, true);
    editor.current_mut().0.selections = SelectionSet::new(12);
    editor.follow_cursor();
    assert_eq!(editor.window().top_row(&editor.current().1.display()), 0);
    let moved = editor.set_preedit(Some(Preedit::new("xyz".into(), None)));
    assert!(moved, "the caret's row was below the view");
    let mut screen = Screen::default();
    editor.screen(&mut screen);
    assert_eq!(shape(&screen), [(1, 0, false), (2, 0, true), (2, 5, false)]);
}

#[test]
fn a_buffer_that_may_hold_secrets_is_previewed_with_no_rows_and_no_wrap() {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 100,
    });
    let secret = editor.add_buffer(super::super::test_buffers::secret_buffer());
    editor.set_wraps(secret, true);
    editor.show(secret);
    let scratch = editor.buffer_named(crate::editor::SCRATCH).unwrap();
    editor.show(scratch);
    let names = [".env"].map(|name| Candidate::new(name, "", ""));
    let style = CompletionStyle {
        given_order: true,
        takes_input: true,
        preview: true,
    };
    editor
        .open_completion_as("Switch to buffer:", "buffer", names.into(), style)
        .unwrap();
    editor.follow_cursor();
    assert_eq!(editor.preview(), Some(Preview::Withheld));
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    assert!(screen.rows().is_empty());
    assert!(editor.preview_wrap.is_none());
}
