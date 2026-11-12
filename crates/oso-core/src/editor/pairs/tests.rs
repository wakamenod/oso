//! A pair of windows that scroll together, on the editor (ADR 0122, step 10).
//!
//! Must not depend on: the host's window.

use super::*;
use crate::buffer::Buffer;
use crate::display_map::Preedit;
use crate::editor::SoleWindow;
use crate::frame::Tiling;
use crate::minibuffer::Kind;
use crate::selection::SelectionSet;

/// The bytes a line of `lines` takes.
const LINE: usize = "line 000\n".len();

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i:03}\n")).collect()
}

/// An editor on `text` in a frame of `rows` by `columns`.
fn on(text: &str, (rows, columns): (usize, usize)) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
    editor.set_frame_size(crate::frame::FrameSize { rows, columns });
    editor
}

fn ids(editor: &Editor) -> Vec<WindowId> {
    editor.splits.iter().collect()
}

fn selected(editor: &Editor) -> WindowId {
    editor.window().id()
}

/// A window on `lines(200)` paired side by side with one on `other`, in a frame of 24 by 161,
/// with `map`. Returns the editor, the first window and the second.
fn pair(other: &str, map: RowMap) -> (Editor, WindowId, WindowId) {
    let mut editor = on(&lines(200), (24, 161));
    let buffer = editor.add_buffer(Buffer::from_text("other", other));
    let first = selected(&editor);
    let second = editor.pair_window(Way::Across, buffer, map).unwrap();
    (editor, first, second)
}

fn window(editor: &Editor, id: WindowId) -> &crate::window::Window {
    editor.window_by_id(id).unwrap()
}

/// The window's top line and its primary cursor's line and display column.
fn place(editor: &Editor, id: WindowId) -> (usize, usize, usize) {
    let window = window(editor, id);
    let display = editor.shown(window).display();
    let top = display.offset_to_display_point(window.scroll().start).row;
    let cursor = window.cursor_point(&display);
    (top, cursor.row, cursor.column)
}

/// Puts the selected window's cursor at `offset`, as a command would, and ends the command.
fn go(editor: &mut Editor, offset: usize) {
    editor.current_mut().0.selections = SelectionSet::new(offset);
    editor.follow_cursor();
}

#[test]
fn a_pair_shows_the_new_buffer_beside_and_keeps_the_first_selected() {
    let (editor, first, second) = pair(&lines(200), RowMap::same());
    assert_eq!(ids(&editor), [first, second]);
    assert_eq!(selected(&editor), first);
    assert_eq!(editor.pair_of(first), Some(second));
    assert_eq!(editor.pair_of(second), Some(first));
    assert_ne!(
        window(&editor, first).buffer(),
        window(&editor, second).buffer()
    );
    let (a, b) = (
        editor.regions(first).unwrap().outer(),
        editor.regions(second).unwrap().outer(),
    );
    assert_eq!((a.columns, b.columns), (81, 80));
    assert_eq!(editor.splits.pairs(), [[first, second]]);
}

#[test]
fn a_pair_is_refused_for_a_paired_window_a_missing_buffer_or_no_room() {
    let (mut editor, _, second) = pair("x\n", RowMap::same());
    let other = window(&editor, second).buffer();
    assert_eq!(
        editor.pair_window(Way::Down, other, RowMap::same()),
        Err(NoPair::Paired)
    );
    let mut small = on("a\n", (24, 20));
    let buffer = small.add_buffer(Buffer::from_text("b", "b\n"));
    assert_eq!(
        small.pair_window(Way::Across, buffer, RowMap::same()),
        Err(NoPair::TooSmall)
    );
    assert_eq!(
        small.pair_window(Way::Down, BufferId(999), RowMap::same()),
        Err(NoPair::NoBuffer)
    );
    assert_eq!(small.windows().len(), 1);
}

#[test]
fn moving_in_one_window_moves_the_other_to_the_aligned_line_and_column() {
    let (mut editor, first, second) = pair(&lines(200), RowMap::same());
    go(&mut editor, 150 * LINE + 4);
    let (top, line, column) = place(&editor, first);
    assert!(top > 0, "the first scrolled");
    assert_eq!(place(&editor, second), (top, line, column));
    assert_eq!(window(&editor, second).selections.len(), 1);
    // The other way round, once the second is selected.
    editor.select_window(second);
    go(&mut editor, 20 * LINE);
    assert_eq!(place(&editor, first), place(&editor, second));
    assert_eq!(place(&editor, first).1, 20);
}

#[test]
fn the_row_map_says_which_line_is_which() {
    // The second has 10 more lines at its start.
    let map = RowMap::new(vec![(0, 10, 200)]).unwrap();
    let (mut editor, first, second) = pair(&lines(210), map);
    go(&mut editor, 100 * LINE + 2);
    let (top, line, column) = place(&editor, first);
    assert_eq!(place(&editor, second), (top + 10, line + 10, column));
    // A new map moves them in step at once.
    assert!(editor.set_row_map(second, RowMap::same()));
    assert_eq!(place(&editor, second), (top, line, column));
    assert!(!editor.set_row_map(WindowId::new(99), RowMap::same()));
}

#[test]
fn a_short_line_keeps_the_column_as_the_goal_and_the_end_of_the_text_clamps() {
    // The second's line 3 is short, and it has 5 lines.
    let (mut editor, first, second) = pair("aaaaaaaa\naa\nb\nc\nd\n", RowMap::same());
    go(&mut editor, 2 * LINE + 6);
    let (_, line, column) = place(&editor, second);
    assert_eq!((line, column), (2, 1), "clamped to the short line");
    let goal = window(&editor, second).selections.primary().goal;
    assert_eq!(goal, Some(Goal::Column(6)), "the column aimed at stays");
    go(&mut editor, 150 * LINE);
    assert_eq!(place(&editor, second).1, 5, "the last line");
    let _ = first;
}

#[test]
fn scrolling_a_screen_scrolls_both() {
    let (mut editor, first, second) = pair(&lines(200), RowMap::same());
    editor.scroll_lines(40).unwrap();
    editor.follow_cursor();
    assert_eq!(place(&editor, first).0, 40);
    assert_eq!(place(&editor, second), place(&editor, first));
}

#[test]
fn the_wheel_scrolls_both_and_moves_the_others_cursors_into_view() {
    let (mut editor, first, second) = pair(&lines(200), RowMap::same());
    assert!(editor.scroll_view(second, 30));
    assert_eq!(place(&editor, first).0, 30);
    assert_eq!(
        place(&editor, first).1,
        30,
        "into view, as the wheel moves any window's"
    );
    assert_eq!(selected(&editor), first, "nothing selected");
    // Sideways too, over lines long enough.
    let long: String = (0..50).map(|_| "x".repeat(300) + "\n").collect();
    let mut editor = on(&long, (24, 161));
    let other = editor.add_buffer(Buffer::from_text("other", &long));
    let first = selected(&editor);
    let second = editor
        .pair_window(Way::Across, other, RowMap::same())
        .unwrap();
    assert!(editor.scroll_view_sideways(first, 12));
    assert_eq!(window(&editor, second).scroll().left, 12);
}

#[test]
fn beneath_a_prompt_the_wheel_leaves_the_cursors_of_the_window_it_came_from() {
    let (mut editor, first, second) = pair(&lines(200), RowMap::same());
    editor
        .open_prompt(
            "Go to line: ",
            Kind::Number {
                history: Some("line"),
            },
        )
        .unwrap();
    assert!(editor.scroll_view(second, 30));
    assert_eq!(place(&editor, first).0, 30);
    assert_eq!(place(&editor, first).1, 0, "what the prompt acts on stays");
    // Nor does a command in the prompt move the pair.
    editor.follow_cursor();
    assert_eq!(place(&editor, first).1, 0);
    let mut editor = pair(&lines(200), RowMap::same()).0;
    editor.set_preedit(Some(Preedit::new("か".into(), None)));
    let second = editor.pair_of(selected(&editor)).unwrap();
    assert!(editor.scroll_view(second, 30));
    assert_eq!(
        place(&editor, selected(&editor)).1,
        0,
        "a composition stays"
    );
}

#[test]
fn isearch_in_one_window_moves_the_other() {
    let (mut editor, first, second) = pair(&lines(200), RowMap::same());
    editor.isearch_begin(false, false);
    editor.isearch_add("line 120");
    editor.follow_cursor();
    assert_eq!(place(&editor, first).1, 120);
    assert_eq!(place(&editor, second).1, 120);
}

#[test]
fn deleting_either_window_closes_both_and_a_pair_alone_stays() {
    let mut editor = on(&lines(200), (24, 161));
    let first = selected(&editor);
    let lower = editor.split_window(Way::Down, false).unwrap();
    let buffer = editor.add_buffer(Buffer::from_text("other", "x\n"));
    let second = editor
        .pair_window(Way::Across, buffer, RowMap::same())
        .unwrap();
    editor.select_window(second);
    editor.delete_window().unwrap();
    assert_eq!(ids(&editor), [lower]);
    assert_eq!(selected(&editor), lower);
    assert_eq!((editor.pair_of(first), editor.pairs.len()), (None, 0));
    let (mut editor, first, second) = pair("x\n", RowMap::same());
    assert_eq!(editor.delete_window(), Err(SoleWindow));
    assert_eq!(ids(&editor), [first, second]);
}

#[test]
fn deleting_the_other_windows_keeps_the_pair() {
    let (mut editor, first, second) = pair("x\n", RowMap::same());
    editor.select_window(second);
    let third = editor.split_window(Way::Down, false).unwrap();
    assert_eq!(ids(&editor), [first, second, third]);
    assert!(editor.delete_other_windows());
    assert_eq!(ids(&editor), [first, second]);
    assert_eq!(editor.pair_of(second), Some(first));
    assert!(!editor.delete_other_windows(), "the pair is all there is");
}

#[test]
fn splitting_a_window_of_a_pair_puts_the_new_one_outside_it() {
    let (mut editor, first, second) = pair("x\n", RowMap::same());
    let new = editor.split_window(Way::Down, false).unwrap();
    assert_eq!(editor.pair_of(first), Some(second));
    let below = editor.regions(new).unwrap().outer();
    assert_eq!(
        (below.column, below.columns),
        (0, 161),
        "across the whole pair"
    );
}

#[test]
fn another_buffer_in_either_window_breaks_the_pair_and_both_stay() {
    let (mut editor, first, second) = pair("x\n", RowMap::same());
    let third = editor.add_buffer(Buffer::from_text("third", "y\n"));
    editor.show(third);
    assert_eq!(
        (editor.pair_of(first), editor.pair_of(second)),
        (None, None)
    );
    assert_eq!(ids(&editor), [first, second]);
    assert!(editor.splits.pairs().is_empty());
    // Showing what it was paired with again keeps it.
    let (mut editor, first, second) = pair("x\n", RowMap::same());
    editor.show(window(&editor, first).buffer());
    assert_eq!(editor.pair_of(first), Some(second));
    // Killing the second's buffer sends it elsewhere, which breaks it.
    editor.kill_buffer(window(&editor, second).buffer());
    assert_eq!(editor.pair_of(first), None);
    assert_eq!(ids(&editor), [first, second]);
}

#[test]
fn unpairing_keeps_one_window_in_the_pairs_place() {
    let (mut editor, first, _) = pair("x\n", RowMap::same());
    assert!(editor.unpair(first));
    assert!(!editor.unpair(first));
    assert_eq!(ids(&editor), [first]);
    assert_eq!(editor.regions(first).unwrap().outer().columns, 161);
    // Keeping the second, which then is selected.
    let (mut editor, _, second) = pair("x\n", RowMap::same());
    assert!(editor.unpair(second));
    assert_eq!((ids(&editor), selected(&editor)), (vec![second], second));
}

#[test]
fn a_pair_turns_stacked_and_stays_in_step() {
    let (mut editor, first, second) = pair(&lines(200), RowMap::same());
    go(&mut editor, 90 * LINE);
    assert_eq!(editor.turn_pair(second, Way::Down), Ok(true));
    assert_eq!(editor.turn_pair(second, Way::Down), Ok(false));
    let (a, b) = (
        editor.regions(first).unwrap().outer(),
        editor.regions(second).unwrap().outer(),
    );
    assert_eq!((a.row, b.row, a.columns), (0, 12, 161), "the first above");
    go(&mut editor, 150 * LINE);
    let ((top, line, _), (other_top, other_line, _)) =
        (place(&editor, first), place(&editor, second));
    assert_eq!(other_line, line);
    // The second is a row shorter, so it scrolls a row more to show its cursor.
    assert_eq!((top, other_top), (140, 141));
    assert_eq!(editor.turn_pair(first, Way::Across), Ok(true));
}

#[test]
fn a_restart_hands_a_pair_over_as_two_windows_side_by_side() {
    let (editor, _, _) = pair("x\n", RowMap::same());
    assert_eq!(
        editor.tiling(),
        Tiling::Run {
            way: Way::Across,
            children: vec![(1, Tiling::Window), (1, Tiling::Window)],
        }
    );
    let mut taken = on("x\n", (24, 161));
    let windows = taken.set_tiling(&editor.tiling()).unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!(
        taken.pair_of(windows[0]),
        None,
        "the owner pairs them again"
    );
}

#[test]
fn a_wrapped_window_of_a_pair_keeps_its_cursor_in_view() {
    // The second's lines wrap into four rows each at its 80 columns.
    let long: String = (0..200)
        .map(|i| format!("{i:03} {}\n", "w".repeat(300)))
        .collect();
    let (mut editor, _, second) = pair(&long, RowMap::same());
    let wrapped = window(&editor, second).buffer();
    assert!(editor.set_wraps(wrapped, true));
    go(&mut editor, 120 * LINE);
    let window = window(&editor, second);
    let display = editor.shown(window).display();
    let (top, row) = (window.top_row(&display), window.cursor_row(&display));
    assert!(
        (top..top + window.size().rows).contains(&row),
        "{row} in view from {top}"
    );
}

/// What moving in step costs at a command's end, over two texts of about 1 GB and a map of a
/// million runs, against the same two windows unpaired. Run it by hand in release:
/// `cargo test --release -p oso-core moving_in_step_costs -- --ignored --nocapture`.
#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(
    clippy::disallowed_macros,
    reason = "a timing run by hand prints what it measured"
)]
fn moving_in_step_costs_little_at_a_commands_end() {
    use std::time::{Duration, Instant};
    const LINES: usize = 16 << 20;
    let text: String = (0..LINES)
        .map(|i| format!("{i:08} {}\n", "x".repeat(54)))
        .collect();
    let runs: Vec<(usize, usize, usize)> =
        (0..1 << 20).map(|i| (i * 16, i * 16 + i % 2, 15)).collect();
    // Each within a tenth of the 8 ms a key may take to show, at p99, in a release build.
    let percentiles = |mut times: Vec<Duration>| {
        times.sort();
        let at = |p: usize| times[(times.len() - 1) * p / 100];
        assert!(cfg!(debug_assertions) || at(99) < Duration::from_micros(800));
        format!("p50 {:?} p99 {:?} max {:?}", at(50), at(99), at(100))
    };
    let line = text.len() / LINES;
    let mut seed: usize = 7;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % (LINES - 1)
    };
    for paired in [false, true] {
        let mut editor = on(&text, (60, 241));
        let buffer = editor.add_buffer(Buffer::from_text("other", &text));
        if paired {
            let started = Instant::now();
            let map = RowMap::new(runs.clone()).unwrap();
            editor.pair_window(Way::Across, buffer, map).unwrap();
            eprintln!("pairing with a map of 2^20 runs: {:?}", started.elapsed());
            let started = Instant::now();
            let first = selected(&editor);
            editor.set_row_map(first, RowMap::new(runs.clone()).unwrap());
            eprintln!("a new map, its runs built: {:?}", started.elapsed());
        } else {
            editor.split_window(Way::Across, false).unwrap();
            let other = editor.pair_of(selected(&editor));
            assert_eq!(other, None);
        }
        let mut moves = Vec::new();
        let mut steps = Vec::new();
        for i in 0..2000 {
            // Far jumps and single lines, as goto-line and C-n do.
            let target = if i % 2 == 0 { next() } else { 0 };
            let offset = if target == 0 {
                let at = editor.window().selections.primary().cursor;
                (at + line).min(text.len() - 1)
            } else {
                target * line + 3
            };
            editor.current_mut().0.selections = SelectionSet::new(offset);
            let started = Instant::now();
            editor.follow_cursor();
            let took = started.elapsed();
            if target == 0 {
                steps.push(took)
            } else {
                moves.push(took)
            }
        }
        let mut wheel = Vec::new();
        let id = selected(&editor);
        for i in 0..500 {
            let started = Instant::now();
            editor.scroll_view(id, if i % 2 == 0 { 3 } else { -2 });
            wheel.push(started.elapsed());
        }
        let name = if paired { "paired" } else { "unpaired" };
        eprintln!("{name}: a far move {}", percentiles(moves));
        eprintln!("{name}: a line down {}", percentiles(steps));
        eprintln!("{name}: a wheel turn {}", percentiles(wheel));
    }
}
