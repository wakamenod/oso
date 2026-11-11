//! Splitting, deleting, selecting and resizing the tiled windows on the editor, and what each
//! window keeps of its own over a shared buffer (ADR 0112).
//!
//! Must not depend on: the host's window.

use std::path::PathBuf;

use super::*;
use crate::buffer::{Buffer, Replacement};
use crate::editor::{Status, VisualAids};
use crate::frame::{FrameSize, Side, Way};
use crate::selection::{MarkActive, Selection, SelectionSet};
use crate::window::{Screen, Scroll};

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i:03}\n")).collect()
}

/// An editor on `text` in a frame of `rows` by `columns`.
fn editor(text: &str, (rows, columns): (usize, usize)) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
    editor.set_frame_size(FrameSize { rows, columns });
    editor
}

fn ids(editor: &Editor) -> Vec<WindowId> {
    editor.splits.iter().collect()
}

fn selected(editor: &Editor) -> WindowId {
    editor.window().id()
}

fn status(editor: &Editor, id: WindowId) -> Status {
    let mut status = Status::default();
    editor.status_into(id, &mut status);
    status
}

fn screen(editor: &Editor, id: WindowId) -> Screen {
    let mut screen = Screen::default();
    editor.screen_of(id, &mut screen);
    screen
}

#[test]
fn a_split_below_keeps_the_original_selected_and_shares_its_rows() {
    let mut editor = editor(&lines(100), (24, 80));
    let first = selected(&editor);
    let new = editor.split_window(Way::Down, false).unwrap();
    assert_ne!(new, first);
    assert_eq!(ids(&editor), [first, new]);
    assert_eq!(selected(&editor), first, "the original stays selected");
    let (top, bottom) = (
        *editor.regions(first).unwrap(),
        *editor.regions(new).unwrap(),
    );
    // 23 rows above the echo area: 12 and 11, each with its modeline.
    assert_eq!((top.text.rows, bottom.text.rows), (11, 10));
    assert_eq!(bottom.outer().row, top.outer().row + top.outer().rows);
    assert_eq!(editor.window().size().rows, 11, "its own text area");
    assert_eq!(editor.window_by_id(new).unwrap().size().rows, 10);
    assert_eq!(editor.windows().len(), 2);
}

#[test]
fn a_split_right_puts_a_divider_on_the_left_window_and_can_select_the_new_one() {
    let mut editor = editor(&lines(10), (24, 81));
    let first = selected(&editor);
    let new = editor.split_window(Way::Across, true).unwrap();
    assert_eq!(selected(&editor), new, "…-and-select");
    let (left, right) = (
        *editor.regions(first).unwrap(),
        *editor.regions(new).unwrap(),
    );
    assert!(left.divider.is_some() && right.divider.is_none());
    assert_eq!(left.text.columns + 1 + right.text.columns, 81);
    assert_eq!(right.text.column, left.outer().columns);
}

#[test]
fn the_new_window_has_the_primary_cursor_its_mark_ring_region_and_view_but_no_other_cursors() {
    let mut editor = editor(&lines(100), (41, 80));
    let row = "line 000\n".len();
    let cursors = vec![Selection::at(row * 30), Selection::at(row * 40 + 2)];
    let mut set = SelectionSet::from_parts(cursors, 1).unwrap();
    set.push_primary_mark(row * 35);
    set.push_marks();
    set.set_mark_active(MarkActive::Active);
    editor.current_mut().0.selections = set.clone();
    editor.follow_cursor();
    let scroll = editor.window().scroll();
    let new = editor.split_window(Way::Down, false).unwrap();
    let window = editor.window_by_id(new).unwrap();
    assert_eq!(window.buffer(), editor.window().buffer());
    assert_eq!(window.selections.len(), 1);
    assert_eq!(window.selections.primary(), set.primary());
    assert_eq!(window.selections.mark_ring(), set.mark_ring());
    assert!(window.selections.mark_active().is_active());
    // Halves of the same height follow the cursor alike.
    assert_eq!(window.scroll(), editor.window().scroll(), "the same view");
    assert_ne!(scroll, Scroll::default());
    assert_eq!(
        editor.window().selections.len(),
        2,
        "the original keeps its own"
    );
}

#[test]
fn a_split_too_small_is_refused_and_changes_nothing() {
    // 6 rows above the echo area: halves of 3 and 3 keep 2 text rows each, but 5 do not.
    let mut editor = editor(&lines(10), (7, 80));
    assert!(editor.split_window(Way::Down, false).is_ok());
    let before = ids(&editor);
    assert_eq!(editor.split_window(Way::Down, false), Err(TooSmall));
    assert_eq!(ids(&editor), before);
    assert_eq!(editor.windows().len(), 2);
    // 21 columns: 11 and 10 with the divider leave 10 and 10 text columns, but 20 do not.
    let mut editor = editor_on(20);
    assert_eq!(editor.split_window(Way::Across, false), Err(TooSmall));
    let mut editor = editor_on(21);
    assert!(editor.split_window(Way::Across, false).is_ok());
}

fn editor_on(columns: usize) -> Editor {
    editor("text", (24, columns))
}

#[test]
fn deleting_selects_the_window_selected_most_recently_before_it() {
    let mut editor = editor(&lines(10), (40, 80));
    let a = selected(&editor);
    let b = editor.split_window(Way::Down, false).unwrap();
    let c = editor.split_window(Way::Down, false).unwrap();
    editor.select_window(c);
    editor.select_window(b);
    assert_eq!(editor.delete_window(), Ok(()));
    assert_eq!(selected(&editor), c, "c was selected before b");
    assert_eq!(ids(&editor), [a, c]);
    assert!(editor.window_by_id(b).is_none());
    assert_eq!(editor.delete_window(), Ok(()));
    assert_eq!(selected(&editor), a);
    assert_eq!(editor.delete_window(), Err(SoleWindow));
    assert_eq!(SoleWindow.to_string(), "Attempt to delete the sole window");
    assert_eq!(
        editor.regions(a).unwrap().text.rows,
        38,
        "the whole tiled area"
    );
}

#[test]
fn deleting_the_other_windows_leaves_the_selected_one_alone_and_the_sole_one_as_it_is() {
    let mut editor = editor(&lines(10), (40, 80));
    let a = selected(&editor);
    let b = editor.split_window(Way::Across, false).unwrap();
    editor.split_window(Way::Down, false).unwrap();
    editor.select_window(b);
    editor.delete_other_windows();
    assert_eq!(ids(&editor), [b]);
    assert_eq!(editor.windows().len(), 1);
    assert!(editor.window_by_id(a).is_none());
    assert_eq!(editor.regions(b).unwrap().text.columns, 80);
    let before = editor.frame().clone();
    editor.delete_other_windows();
    assert_eq!(*editor.frame(), before);
}

#[test]
fn other_window_goes_in_the_trees_order_and_wraps_around_both_ways() {
    let mut editor = editor(&lines(10), (40, 80));
    let a = selected(&editor);
    let c = editor.split_window(Way::Down, false).unwrap();
    // The new window goes right after the one split, before `c`.
    let b = editor.split_window(Way::Across, false).unwrap();
    assert_eq!(ids(&editor), [a, b, c]);
    editor.other_window(1);
    assert_eq!(selected(&editor), b);
    editor.other_window(2);
    assert_eq!(selected(&editor), a, "around the end");
    editor.other_window(-1);
    assert_eq!(selected(&editor), c, "back past the start");
    editor.other_window(0);
    assert_eq!(selected(&editor), c);
    editor.other_window(isize::MIN);
    editor.other_window(isize::MAX);
    assert!(ids(&editor).contains(&selected(&editor)));
}

#[test]
fn other_window_skips_a_window_the_frame_has_no_room_for() {
    let mut editor = editor(&lines(10), (12, 80));
    let a = selected(&editor);
    let b = editor.split_window(Way::Down, false).unwrap();
    let c = editor.split_window(Way::Down, false).unwrap();
    assert_eq!(ids(&editor), [a, c, b]);
    // 4 rows above the echo area hold two windows of a text row and a modeline.
    editor.set_frame_size(FrameSize {
        rows: 5,
        columns: 80,
    });
    assert_eq!(
        editor.regions(b).unwrap().outer().rows,
        0,
        "the last goes first"
    );
    editor.other_window(1);
    assert_eq!(selected(&editor), c);
    editor.other_window(1);
    assert_eq!(selected(&editor), a, "past b");
    // Selected anyway, it takes the cells of another.
    editor.select_window(b);
    assert_eq!(selected(&editor), b);
    assert!(editor.regions(b).unwrap().outer().rows > 0);
}

#[test]
fn windmove_selects_the_window_on_that_side_at_the_cursors_row_or_column() {
    let mut editor = editor(&lines(30), (40, 81));
    let a = selected(&editor);
    let b = editor.split_window(Way::Across, false).unwrap();
    editor.select_window(b);
    let c = editor.split_window(Way::Down, false).unwrap();
    // a | b
    //   | c
    assert_eq!(editor.windmove(Side::Right), Err(NoWindow(Side::Right)));
    assert_eq!(
        NoWindow(Side::Right).to_string(),
        "No window right from selected window"
    );
    assert_eq!(editor.windmove(Side::Down), Ok(()));
    assert_eq!(selected(&editor), c);
    assert_eq!(editor.windmove(Side::Up), Ok(()));
    assert_eq!(selected(&editor), b);
    assert_eq!(editor.windmove(Side::Left), Ok(()));
    assert_eq!(selected(&editor), a);
    // From the bottom of `a`, its cursor's row is beside `c`.
    let row = "line 000\n".len();
    editor.current_mut().0.selections = SelectionSet::new(row * 25);
    editor.follow_cursor();
    assert_eq!(editor.windmove(Side::Right), Ok(()));
    assert_eq!(selected(&editor), c);
    assert_eq!(editor.windmove(Side::Down), Err(NoWindow(Side::Down)));
    assert_eq!(
        NoWindow(Side::Up).to_string(),
        "No window up from selected window"
    );
}

#[test]
fn resizing_takes_cells_from_the_neighbours_and_says_when_nothing_moved() {
    let mut editor = editor(&lines(30), (40, 80));
    let a = selected(&editor);
    let b = editor.split_window(Way::Down, false).unwrap();
    let rows = |editor: &Editor, id| editor.regions(id).unwrap().text.rows;
    let (before_a, before_b) = (rows(&editor, a), rows(&editor, b));
    assert_eq!(editor.resize_window(Way::Down, 3), Ok(()));
    assert_eq!(rows(&editor, a), before_a + 3);
    assert_eq!(rows(&editor, b), before_b - 3);
    assert_eq!(
        editor.window().size().rows,
        before_a + 3,
        "the view's size too"
    );
    assert_eq!(editor.resize_window(Way::Down, -5), Ok(()));
    assert_eq!(rows(&editor, a), before_a - 2);
    assert_eq!(editor.resize_window(Way::Down, 0), Ok(()));
    assert_eq!(
        editor.resize_window(Way::Across, 1),
        Err(CannotResize::Enlarge),
        "nothing beside it"
    );
    assert_eq!(
        editor.resize_window(Way::Across, -1),
        Err(CannotResize::Shrink)
    );
    assert_eq!(
        CannotResize::Enlarge.to_string(),
        "Cannot enlarge the window"
    );
    assert_eq!(CannotResize::Shrink.to_string(), "Cannot shrink the window");
    // As far as the other keeps the smallest a window may be, and no further.
    assert_eq!(editor.resize_window(Way::Down, isize::MAX), Ok(()));
    assert_eq!(rows(&editor, b), 2);
    assert_eq!(
        editor.resize_window(Way::Down, 1),
        Err(CannotResize::Enlarge)
    );
    assert!(editor.balance_windows());
    assert_eq!(rows(&editor, a).abs_diff(rows(&editor, b)), 1);
    assert!(!editor.balance_windows(), "already balanced");
}

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
fn selecting_another_window_flashes_and_a_delete_is_not_mistaken_for_it() {
    let mut editor = editor("text", (24, 80));
    let other = editor.split_window(Way::Down, false).unwrap();
    let before = editor.view_sample();
    editor.select_window(other);
    assert!(editor.flashes(&before, &editor.view_sample()));
    // Deleting it selects the first again, which is another window than `other`.
    let before = editor.view_sample();
    editor.delete_window().unwrap();
    assert!(editor.flashes(&before, &editor.view_sample()));
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
fn window_commands_inside_a_prompt_keep_the_window_beneath_by_its_name() {
    let mut editor = editor("text", (24, 80));
    let first = selected(&editor);
    let other = editor.split_window(Way::Down, false).unwrap();
    editor.select_window(other);
    editor
        .open_prompt("Name: ", crate::minibuffer::Kind::Text { history: None })
        .unwrap();
    assert_eq!(editor.tiled().0.id(), other);
    editor.quit();
    assert_eq!(selected(&editor), other);
    assert!(editor.window_by_id(first).is_some());
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
