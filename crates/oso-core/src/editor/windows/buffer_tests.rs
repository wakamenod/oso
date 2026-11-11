//! What each window keeps of its own over a shared buffer, and what only the selected one
//! shows: edits and undo seen in both, wraps and their one budget, a buffer killed or never
//! shown before, the visual aids, the gutter and buffers that may hold secrets (ADR 0112).
//!
//! Must not depend on: the host's window.

use std::path::PathBuf;

use super::tests::{editor, lines, screen, selected, status};
use crate::buffer::{Buffer, Replacement};
use crate::editor::{Editor, VisualAids};
use crate::frame::Way;
use crate::selection::{MarkActive, SelectionSet};

#[test]
fn an_edit_in_one_window_moves_the_others_cursor_and_it_follows_it_when_the_command_ends() {
    let row = "line 000\n".len();
    let mut editor = editor(&lines(100), (24, 80));
    editor.current_mut().0.selections = SelectionSet::new(row * 50);
    editor.follow_cursor();
    let other = editor.split_window(Way::Down, false).unwrap();
    // The selected window goes to the top, and deletes the 40 lines above the other's
    // cursor and top.
    editor.current_mut().0.selections = SelectionSet::new(0);
    editor.follow_cursor();
    let id = editor.window().buffer();
    editor
        .apply_edits(
            id,
            &[Replacement {
                range: 0..row * 40,
                text: "",
            }],
        )
        .unwrap();
    let window = editor.window_by_id(other).unwrap();
    assert_eq!(window.selections.primary().cursor, row * 10, "as a marker");
    editor.follow_cursor();
    let window = editor.window_by_id(other).unwrap();
    let display = editor.shown(window).display();
    let (top, cursor) = (window.top_row(&display), window.cursor_row(&display));
    assert!(
        top <= cursor && cursor < top + window.size().rows,
        "in view"
    );
}

#[test]
fn a_window_whose_top_went_past_the_end_follows_its_cursor() {
    let row = "line 000\n".len();
    let mut editor = editor(&lines(100), (24, 80));
    editor.current_mut().0.selections = SelectionSet::new(row * 90);
    editor.follow_cursor();
    let other = editor.split_window(Way::Down, false).unwrap();
    editor.current_mut().0.selections = SelectionSet::new(0);
    editor.follow_cursor();
    let id = editor.window().buffer();
    let end = editor.current().1.text().len();
    let edit = Replacement {
        range: row * 10..end,
        text: "",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    editor.follow_cursor();
    let window = editor.window_by_id(other).unwrap();
    let display = editor.shown(window).display();
    assert!(window.top_row(&display) <= window.cursor_row(&display));
}

#[test]
fn each_window_wraps_at_its_own_width_and_they_share_the_commands_budget() {
    let mut editor = editor(&"word ".repeat(400), (24, 81));
    let id = editor.window().buffer();
    let other = editor.split_window(Way::Across, false).unwrap();
    editor.select_window(other);
    editor.resize_window(Way::Across, 10).unwrap();
    assert!(editor.set_wraps(id, true));
    let columns: Vec<usize> = editor
        .windows()
        .filter(|window| window.wrap().is_some())
        .map(|window| window.size().columns)
        .collect();
    assert_eq!(columns.len(), 2, "both wrap");
    assert_ne!(columns[0], columns[1]);
    editor.begin_undo_group();
    let spent: usize = editor
        .windows
        .iter()
        .filter_map(|window| window.wrap_map())
        .map(crate::display_map::wrap_map::WrapMap::budget_left)
        .sum();
    assert_eq!(
        spent,
        crate::display_map::wrap_map::COMMAND_BUDGET,
        "one budget"
    );
    let held: Vec<usize> = editor
        .windows
        .iter()
        .map(|window| window.wrap_map().unwrap().budget_left())
        .collect();
    let index = editor.selected;
    assert_eq!(held[index], spent, "the selected window first");
    editor.end_undo_group();
}

#[test]
fn a_buffer_a_window_never_showed_starts_where_the_window_selected_last_left_it() {
    let row = "line 000\n".len();
    let mut editor = editor("scratch", (41, 80));
    let file = editor.add_buffer(Buffer::from_text("f", &lines(100)));
    let other = editor.split_window(Way::Down, false).unwrap();
    editor.show(file);
    editor.current_mut().0.selections = SelectionSet::new(row * 60);
    editor.follow_cursor();
    editor.select_window(other);
    editor.show(file);
    assert_eq!(editor.window().selections.primary().cursor, row * 60);
    let first = editor.splits.iter().next().unwrap();
    let start = editor.window_by_id(first).unwrap().scroll().start;
    assert_eq!(editor.window().scroll().start, start);
}

#[test]
fn killing_a_buffer_shown_twice_moves_both_windows_and_deletes_none() {
    let mut editor = editor("scratch", (40, 80));
    let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/b")));
    editor.show(file);
    let other = editor.split_window(Way::Across, false).unwrap();
    editor.kill_buffer(file);
    assert_eq!(editor.windows().len(), 2);
    assert!(editor.windows().all(|window| window.buffer() != file));
    assert!(editor.window_by_id(other).is_some());
}

#[test]
fn only_the_selected_window_has_a_region_a_band_a_flash_and_isearchs_matches() {
    let mut editor = editor("abc abc\n", (24, 80));
    editor
        .current_mut()
        .0
        .selections
        .set_mark_active(MarkActive::Active);
    editor.current_mut().0.selections = {
        let mut set = SelectionSet::new(3);
        set.push_primary_mark(0);
        set.set_mark_active(MarkActive::Active);
        set
    };
    let me = selected(&editor);
    let other = editor.split_window(Way::Down, false).unwrap();
    for (id, has) in [(me, true), (other, false)] {
        let screen = screen(&editor, id);
        let mut cells = Vec::new();
        editor.region_cells(id, &screen, &mut cells);
        assert_eq!(!cells.is_empty(), has, "region");
        assert_eq!(
            editor.current_line_cells(id, &screen).is_some(),
            has,
            "band"
        );
        assert_eq!(editor.flash_cells(id, &screen).is_some(), has, "flash");
        assert_eq!(status(&editor, id).selected, has);
    }
    editor.isearch_begin(false);
    editor.isearch_add("abc");
    let mut cells = crate::editor::MatchCells::default();
    editor.isearch_cells(other, &screen(&editor, other), &mut cells);
    assert!(cells.current.is_empty() && cells.others.is_empty());
    editor.isearch_cells(me, &screen(&editor, me), &mut cells);
    assert!(!cells.current.is_empty());
}

#[test]
fn the_agent_segment_is_only_on_the_selected_windows_modeline() {
    let mut editor = editor("text", (24, 80));
    editor.set_agent_channel(crate::editor::AgentChannel::Connected);
    let me = selected(&editor);
    let other = editor.split_window(Way::Down, false).unwrap();
    assert!(!status(&editor, me).agent.is_empty());
    assert!(status(&editor, other).agent.is_empty());
    assert!(!status(&editor, other).agent_connected);
    assert_eq!(status(&editor, other).echo, status(&editor, me).echo);
}

#[test]
fn each_window_has_the_gutter_its_own_buffer_needs() {
    let mut editor = editor("a\n", (24, 81));
    editor.set_visual_aids(VisualAids {
        line_numbers: true,
        ..VisualAids::default()
    });
    let me = selected(&editor);
    let other = editor.split_window(Way::Across, true).unwrap();
    let file = editor.add_buffer(Buffer::from_text("f", &lines(2000)));
    editor.show(file);
    assert_eq!(editor.regions(me).unwrap().gutter.columns, 4);
    assert_eq!(editor.regions(other).unwrap().gutter.columns, 5);
    let mut gutter = crate::editor::Gutter::default();
    editor.gutter_into(me, &screen(&editor, me), &mut gutter);
    assert_eq!(gutter.rows().next(), Some(("  1 ", true)));
}

#[test]
fn a_secret_buffer_in_a_window_not_selected_is_still_shown() {
    let mut editor = editor("text", (24, 80));
    let secret = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/.env")));
    let other = editor.split_window(Way::Down, true).unwrap();
    editor.show(secret);
    assert!(editor.shows_secret());
    editor.other_window(1);
    assert_ne!(selected(&editor), other);
    assert!(editor.shows_secret(), "beside the selected one");
    assert!(
        !editor.current().1.is_secret(),
        "the selected window's is not"
    );
    editor.select_window(other);
    editor.delete_window().unwrap();
    assert!(!editor.shows_secret());
}

#[test]
fn deleting_a_window_that_shows_a_secret_buffer_takes_what_was_said_around_it_along() {
    let echo = |editor: &Editor| status(editor, editor.window().id()).echo;
    for others in [false, true] {
        let mut editor = editor("text", (24, 80));
        let secret = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/.env")));
        let me = selected(&editor);
        let other = editor.split_window(Way::Down, true).unwrap();
        editor.show(secret);
        editor.select_window(me);
        editor.echo("said beside a secret");
        assert_eq!(echo(&editor), "said beside a secret");
        editor.begin_command();
        if others {
            assert!(editor.delete_other_windows());
        } else {
            editor.select_window(other);
            editor.delete_window().unwrap();
        }
        editor.message("said by the command that took it away");
        editor.end_command();
        assert!(!editor.shows_secret());
        assert_eq!(echo(&editor), "", "others: {others}");
    }
}

/// What is left of the command's wrap budget, in the pool and every wrap.
fn budget_left(editor: &Editor) -> usize {
    editor.wrap_pool
        + editor
            .windows
            .iter()
            .filter_map(|window| window.wrap_map())
            .map(crate::display_map::wrap_map::WrapMap::budget_left)
            .sum::<usize>()
}

#[test]
fn new_wraps_in_several_windows_share_one_budget() {
    let budget = crate::display_map::wrap_map::COMMAND_BUDGET;
    let mut editor = editor("ab\n", (24, 81));
    let id = editor.window().buffer();
    editor.split_window(Way::Across, false).unwrap();
    editor.split_window(Way::Down, false).unwrap();
    editor.begin_undo_group();
    assert_eq!(budget_left(&editor), budget);
    // Each window makes a wrap here, which used to come with a budget of its own.
    assert!(editor.set_wraps(id, true));
    assert!(budget_left(&editor) < budget, "spent some");
    editor.end_undo_group();
    editor.begin_undo_group();
    editor.split_window(Way::Down, false).unwrap();
    assert!(
        budget_left(&editor) <= budget,
        "a split's wrap has none of its own"
    );
    editor.end_undo_group();
}

#[test]
fn undo_restores_the_cursors_of_the_window_that_undoes_and_moves_the_others_as_markers() {
    let mut editor = editor("abc\ndef\n", (24, 80));
    let id = editor.window().buffer();
    let other = editor.split_window(Way::Down, false).unwrap();
    editor.select_window(other);
    editor.current_mut().0.selections = SelectionSet::new(6);
    editor.other_window(1);
    editor.current_mut().0.selections = SelectionSet::new(1);
    editor.begin_undo_group();
    let typed = Replacement {
        range: 0..0,
        text: "xy",
    };
    editor.apply_edits(id, &[typed]).unwrap();
    editor.end_undo_group();
    editor.current_mut().0.selections = SelectionSet::new(4);
    let other_cursor = |editor: &Editor| {
        let window = editor.window_by_id(other).unwrap();
        window.selections.primary().cursor
    };
    assert_eq!(other_cursor(&editor), 8);
    editor.begin_undo_group();
    editor.undo().unwrap();
    editor.end_undo_group();
    assert_eq!(editor.current().1.text().to_string(), "abc\ndef\n");
    assert_eq!(
        editor.window().selections.primary().cursor,
        1,
        "where the group began"
    );
    assert_eq!(other_cursor(&editor), 6, "back as a marker");
}

#[test]
fn killing_a_buffer_shown_only_in_the_window_not_selected_moves_only_that_window() {
    let mut editor = editor("scratch", (24, 80));
    let me = selected(&editor);
    let scratch = editor.window().buffer();
    let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/b")));
    let other = editor.split_window(Way::Down, true).unwrap();
    editor.show(file);
    editor.select_window(me);
    editor.kill_buffer(file);
    assert_eq!(selected(&editor), me, "the selection stays");
    assert_eq!(editor.window().buffer(), scratch);
    assert_eq!(editor.window_by_id(other).unwrap().buffer(), scratch);
    assert!(editor.buffer(file).is_none());
}
