//! The wheel in a split frame: it scrolls the window under the pointer without selecting it
//! (ADR 0112).
//!
//! Must not depend on: the host's window.

use super::tests::{editor, lines, selected};
use super::*;
use crate::display_map::Preedit;
use crate::editor::VisualAids;
use crate::frame::{PixelGrid, Way};
use crate::minibuffer::Kind;

fn top(editor: &Editor, id: WindowId) -> usize {
    let window = editor.window_by_id(id).unwrap();
    window.top_row(&editor.shown(window).display())
}

fn cursor(editor: &Editor, id: WindowId) -> usize {
    editor.window_by_id(id).unwrap().selections.primary().cursor
}

/// A window above another, each on its own place in 200 lines, the top one selected.
fn stacked() -> (Editor, WindowId, WindowId) {
    let mut editor = editor(&lines(200), (24, 80));
    let upper = selected(&editor);
    let lower = editor.split_window(Way::Down, false).unwrap();
    (editor, upper, lower)
}

#[test]
fn the_wheel_picks_the_window_whose_text_gutter_or_modeline_is_under_the_pointer() {
    let mut editor = editor(&lines(200), (24, 81));
    editor.set_visual_aids(VisualAids {
        line_numbers: true,
        ..VisualAids::default()
    });
    let left = selected(&editor);
    let right = editor.split_window(Way::Across, false).unwrap();
    let lower = editor.split_window(Way::Down, true).unwrap();
    editor.select_window(left);
    for id in [left, right, lower] {
        let regions = *editor.regions(id).unwrap();
        assert!(regions.gutter.columns > 0, "line numbers are on");
        for part in [regions.text, regions.gutter, regions.modeline] {
            let (row, column) = (part.row + part.rows - 1, part.column + part.columns - 1);
            assert_eq!(editor.wheel_window(Some((part.row, part.column))), id);
            assert_eq!(editor.wheel_window(Some((row, column))), id);
        }
    }
    let divider = editor.regions(left).unwrap().divider.unwrap();
    let echo = editor.frame().echo;
    let size = editor.frame().size();
    for cell in [
        Some((divider.row, divider.column)),
        Some((echo.row, 3)),
        Some((size.rows, 0)),
        Some((0, size.columns)),
        Some((usize::MAX, usize::MAX)),
        None,
    ] {
        assert_eq!(
            editor.wheel_window(cell),
            left,
            "{cell:?}: the selected one"
        );
    }
}

#[test]
fn a_pixel_goes_to_its_cells_window_through_the_grid() {
    let (editor, upper, lower) = stacked();
    let grid = PixelGrid::new(10.0, 20.0);
    let place = |row: usize, column: usize| {
        let [x, y] = grid.cell(row as i64, column as i64);
        // Within the cell, off its corner.
        grid.cell_at(f64::from(x) + 5.0, f64::from(y) + 19.0)
    };
    let below = editor.regions(lower).unwrap().text;
    assert_eq!(editor.wheel_window(place(0, 0)), upper);
    assert_eq!(editor.wheel_window(place(below.row, 40)), lower);
    assert_eq!(
        editor.wheel_window(place(below.row - 1, 79)),
        upper,
        "its modeline"
    );
    assert_eq!(
        editor.wheel_window(grid.cell_at(2.0, 2.0)),
        upper,
        "the margin"
    );
}

#[test]
fn the_wheel_scrolls_a_window_not_selected_and_moves_its_cursors_into_view() {
    let (mut editor, upper, lower) = stacked();
    let rows = editor.window_by_id(lower).unwrap().size().rows;
    assert!(editor.scroll_view(lower, 30));
    assert_eq!(selected(&editor), upper, "nothing is selected");
    assert_eq!(top(&editor, lower), 30);
    assert_eq!(top(&editor, upper), 0);
    let line = "line 000\n".len();
    assert_eq!(cursor(&editor, lower), 30 * line, "the nearest row in view");
    assert_eq!(cursor(&editor, upper), 0);
    assert!(editor.scroll_view(lower, -10));
    assert_eq!(top(&editor, lower), 20);
    assert_eq!(
        cursor(&editor, lower),
        (20 + rows - 1) * line,
        "the last row going up"
    );
}

#[test]
fn the_selected_window_follows_its_cursor_after_another_scrolled() {
    let (mut editor, upper, lower) = stacked();
    assert!(editor.scroll_view(lower, 50));
    assert!(!editor.follow_cursor());
    assert_eq!(top(&editor, lower), 50, "the next command leaves it");
    assert_eq!(top(&editor, upper), 0);
}

#[test]
fn the_wheel_scrolls_sideways_the_window_under_the_pointer() {
    let long = format!("{}\n", "x".repeat(300)).repeat(40);
    let mut editor = editor(&long, (24, 80));
    let upper = selected(&editor);
    let lower = editor.split_window(Way::Down, false).unwrap();
    assert!(editor.scroll_view_sideways(lower, 12));
    let left = |editor: &Editor, id| editor.window_by_id(id).unwrap().scroll().left;
    assert_eq!((left(&editor, upper), left(&editor, lower)), (0, 12));
    assert_eq!(cursor(&editor, lower), 0, "cursors stay sideways");
    assert_eq!(selected(&editor), upper);
}

#[test]
fn beneath_a_prompt_the_wheel_scrolls_any_tiled_window_and_moves_no_cursor_beneath() {
    let (mut editor, upper, lower) = stacked();
    editor
        .open_prompt(
            "Go to line: ",
            Kind::Number {
                history: Some("line"),
            },
        )
        .unwrap();
    assert!(editor.scroll_view(lower, 30));
    assert!(editor.scroll_view(upper, 10));
    assert_eq!((top(&editor, upper), top(&editor, lower)), (10, 30));
    assert_eq!(cursor(&editor, upper), 0, "what the prompt acts on stays");
    let line = "line 000\n".len();
    assert_eq!(
        cursor(&editor, lower),
        30 * line,
        "another window's comes into its view, so it keeps it after the prompt"
    );
    let prompt = editor.window().id();
    assert_eq!(
        editor.wheel_window(None),
        upper,
        "the one beneath where no window is"
    );
    assert!(
        !editor.scroll_view(prompt, 1),
        "the prompt's own window is not tiled"
    );
    assert!(!editor.scroll_view_sideways(prompt, 1));
}

#[test]
fn a_composition_keeps_the_selected_windows_cursors_and_no_others() {
    let (mut editor, upper, lower) = stacked();
    editor.set_preedit(Some(Preedit::new("か".into(), None)));
    assert!(editor.scroll_view(upper, 30));
    assert_eq!(
        cursor(&editor, upper),
        0,
        "the commit goes where it started"
    );
    assert!(editor.scroll_view(lower, 30));
    assert_ne!(cursor(&editor, lower), 0, "it is not composing there");
}

/// Two windows side by side on long wrapped lines, the left one selected or not.
fn wrapped_beside(left_selected: bool) -> (Editor, WindowId) {
    let long = format!("{}\n", "word ".repeat(60)).repeat(400);
    let mut editor = editor(&long, (24, 201));
    let id = editor.window().buffer();
    let left = selected(&editor);
    let right = editor.split_window(Way::Across, false).unwrap();
    if !left_selected {
        editor.select_window(right);
    }
    assert!(editor.set_wraps(id, true));
    (editor, left)
}

#[test]
fn a_wrapped_window_scrolls_by_view_rows_as_far_whether_selected_or_not() {
    let (mut away, left) = wrapped_beside(false);
    let (mut here, same) = wrapped_beside(true);
    assert_eq!(left, same);
    for rows in [7, -5, 30, -1] {
        assert!(away.scroll_view(left, rows));
        assert!(here.scroll_view(left, rows));
        let window = away.window_by_id(left).unwrap();
        assert!(window.rows(&away.shown(window).display()).wrapped());
        assert_eq!(
            window.scroll(),
            here.window_by_id(left).unwrap().scroll(),
            "by {rows}: its own wrap, with the wheel's whole budget"
        );
        assert_eq!(cursor(&away, left), cursor(&here, left));
    }
    assert!(top(&away, left) > 0);
    assert_ne!(selected(&away), left);
}

#[test]
fn the_wheel_on_a_window_deleted_meanwhile_does_nothing() {
    let (mut editor, upper, lower) = stacked();
    editor.select_window(lower);
    editor.delete_window().unwrap();
    assert!(!editor.scroll_view(lower, 3));
    assert!(!editor.scroll_view_sideways(lower, 3));
    assert_eq!(selected(&editor), upper);
    assert_eq!(top(&editor, upper), 0);
}
