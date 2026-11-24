//! Which windows follow a native transcript's end as rows come, and where a window starts in
//! one (ADR 0165, part 1b-1).
//!
//! Must not depend on: the host's threads, files or processes.

use crate::buffer::{Buffer, BufferId};
use crate::complete::Candidate;
use crate::display_map::Preedit;
use crate::editor::{CompletionStyle, Editor, Preview};
use crate::frame::{FrameSize, Way};

use crate::minibuffer::Kind;
use crate::selection::{MarkActive, Selection, SelectionSet};
use crate::window::{DockSlot, WindowId};

/// An editor of 24 rows by 80 whose window shows a transcript of `rows` rows and its end, and
/// that transcript.
fn following(rows: usize) -> (Editor, BufferId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 80,
    });
    let id = editor.add_native_session_buffer("*claude: oso*");
    editor.show(id);
    for row in 0..rows {
        add(&mut editor, id, &format!("row {row}\n"));
    }
    (editor, id)
}

fn len(editor: &Editor, id: BufferId) -> usize {
    editor.buffer(id).unwrap().text().len()
}

/// A row added at the end.
fn add(editor: &mut Editor, id: BufferId, row: &str) {
    let end = len(editor, id);
    editor.replace_native_rows(id, end..end, row, &[]);
}

fn cursor(editor: &Editor, id: WindowId) -> usize {
    let window = editor.window_by_id(id).unwrap();
    window.selections.primary().cursor
}

/// Whether window `id` is at the end of the transcript it shows, its last row in view.
fn at_end(editor: &Editor, id: WindowId) -> bool {
    let window = editor.window_by_id(id).unwrap();
    let buffer = editor.buffer(window.buffer()).unwrap();
    cursor(editor, id) == buffer.text().len() && shows_end(editor, id)
}

/// Whether the last row of the transcript window `id` shows is in view.
fn shows_end(editor: &Editor, id: WindowId) -> bool {
    let window = editor.window_by_id(id).unwrap();
    let display = editor.buffer(window.buffer()).unwrap().display();
    let top = window.top_row(&display);
    let last = window.row_count(&display) - 1;
    top <= last && last < top + window.size().rows
}

fn top(editor: &Editor, id: WindowId) -> usize {
    let window = editor.window_by_id(id).unwrap();
    window.top_row(&editor.buffer(window.buffer()).unwrap().display())
}

fn composer(editor: &mut Editor, id: BufferId) -> WindowId {
    let input = editor.add_buffer(Buffer::from_text("*compose: oso*", ""));
    editor.set_dock(id, DockSlot::Composer, Some(input));
    editor.dock_window(id, DockSlot::Composer).unwrap()
}

/// What follows a range goes with it, though its new text only goes on from the old: a place at the
/// first byte after it stays on that byte, the cursor's and the view's top.
#[test]
fn what_follows_a_row_that_goes_on_keeps_its_place() {
    for (old, new) in [
        ("é\n", "é\n\n"),
        ("a", "ab"),
        ("a\n", "a\nb\n"),
        ("", "a\n"),
    ] {
        let (mut editor, id) = following(0);
        let window = editor.window().id();
        add(&mut editor, id, old);
        for row in 0..60 {
            add(&mut editor, id, &format!("row {row}\n"));
        }
        let mut selections = editor.window().selections.clone();
        selections.move_primary(old.len());
        let scroll = crate::window::Scroll {
            start: old.len(),
            left: 0,
        };
        editor.restore_positions_in(window, selections, scroll);
        editor.replace_native_rows(id, 0..old.len(), new, &[]);
        let after = editor.window_by_id(window).unwrap();
        let wanted = if old.is_empty() { 0 } else { new.len() };
        let at = after.selections.primary().cursor;
        assert_eq!(at, wanted, "{old:?} to {new:?}");
        if old.ends_with('\n') {
            assert_eq!(after.scroll().start, wanted, "{old:?} to {new:?}");
        }
    }
}

#[test]
fn a_transcript_follows_while_its_composer_is_selected() {
    let (mut editor, id) = following(0);
    let transcript = editor.window().id();
    let composer = composer(&mut editor, id);
    editor.select_window(composer);
    for row in 0..60 {
        add(&mut editor, id, &format!("row {row}\n"));
        assert!(at_end(&editor, transcript), "row {row}");
    }
    // Wrapped lines count by the rows they take.
    editor.set_wraps(id, true);
    add(&mut editor, id, &format!("{}\n", "word ".repeat(400)));
    assert!(at_end(&editor, transcript));
    assert_eq!(editor.window().id(), composer, "nothing is selected");
}

#[test]
fn every_window_at_the_end_follows_and_the_others_stay() {
    let (mut editor, id) = following(60);
    let upper = editor.window().id();
    let lower = editor.split_window(Way::Down, false).unwrap();
    let side = editor.split_window(Way::Across, false).unwrap();
    for window in [upper, side] {
        assert!(at_end(&editor, window), "a split keeps the place");
    }
    editor.select_window(lower);
    editor.scroll_view(side, -20);
    let (left, top_before) = (cursor(&editor, side), top(&editor, side));
    let rows = "one\ntwo\n".repeat(30);
    add(&mut editor, id, &rows);
    assert!(at_end(&editor, upper) && at_end(&editor, lower));
    assert_eq!(cursor(&editor, side), left);
    assert_eq!(top(&editor, side), top_before);
    // Nor does the next command take it there.
    editor.follow_cursor();
    assert_eq!(top(&editor, side), top_before);
}

#[test]
fn a_dock_that_grows_and_shrinks_leaves_the_end_in_view() {
    let (mut editor, id) = following(60);
    let transcript = editor.window().id();
    let composer = composer(&mut editor, id);
    editor.select_window(composer);
    let card = editor.add_buffer(Buffer::from_text("*card*", "a\nb\nc\nd\ne\n"));
    editor.set_dock(id, DockSlot::Card, Some(card));
    assert!(at_end(&editor, transcript));
    add(&mut editor, id, "more\nrows\n");
    assert!(at_end(&editor, transcript));
    editor.set_dock(id, DockSlot::Card, None);
    assert!(at_end(&editor, transcript));
}

#[test]
fn a_window_that_never_showed_it_starts_at_its_end() {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 80,
    });
    let id = editor.add_native_session_buffer("*claude: oso*");
    add(&mut editor, id, &"row\n".repeat(60));
    editor.show(id);
    let first = editor.window().id();
    assert!(at_end(&editor, first));
    // Nor where another window is in it.
    let other = editor.add_buffer(Buffer::from_text("notes", "x\n"));
    let second = editor.split_window(Way::Down, false).unwrap();
    editor.show_in(second, other);
    editor.scroll_view(first, -30);
    let third = editor.split_window(Way::Across, false).unwrap();
    editor.show_in(third, other);
    editor.select_window(second);
    let fresh = editor.split_window(Way::Across, true).unwrap();
    assert!(!at_end(&editor, first));
    editor.show_in(fresh, id);
    assert!(at_end(&editor, fresh));
    // A space's first tree too.
    let tree = editor.window_tree_showing(&[id]);
    let _ = editor.swap_window_tree(tree).unwrap();
    assert!(at_end(&editor, editor.window().id()));
}

#[test]
fn a_window_that_comes_back_is_at_the_end_if_it_left_it_there() {
    let (mut editor, id) = following(60);
    let window = editor.window().id();
    let other = editor.add_buffer(Buffer::from_text("notes", "x\n"));
    editor.show(other);
    add(&mut editor, id, &"row\n".repeat(60));
    editor.show(id);
    assert!(at_end(&editor, window), "from its end");
    editor.scroll_view(window, -40);
    let (left, before) = (cursor(&editor, window), top(&editor, window));
    editor.show(other);
    add(&mut editor, id, &"row\n".repeat(60));
    editor.show(id);
    assert_eq!(
        (cursor(&editor, window), top(&editor, window)),
        (left, before)
    );
}

#[test]
fn a_tree_kept_behind_the_frame_comes_back_at_the_end() {
    let (mut editor, id) = following(60);
    let first = editor.window().id();
    let away = editor.split_window(Way::Down, false).unwrap();
    editor.scroll_view(away, -30);
    let left = cursor(&editor, away);
    let other = editor.add_buffer(Buffer::from_text("notes", "x\n"));
    let tree = editor.window_tree_showing(&[other]);
    let kept = editor.swap_window_tree(tree).unwrap();
    add(&mut editor, id, &"row\n".repeat(60));
    editor.swap_window_tree(kept).unwrap();
    assert!(at_end(&editor, first));
    assert_eq!(cursor(&editor, away), left);
    // And one whose window had left it at its end, behind another buffer.
    editor.show_in(first, other);
    let tree = editor.window_tree_showing(&[other]);
    let kept = editor.swap_window_tree(tree).unwrap();
    add(&mut editor, id, &"row\n".repeat(60));
    editor.swap_window_tree(kept).unwrap();
    editor.show_in(first, id);
    assert!(at_end(&editor, first));
}

#[test]
fn scrolling_back_to_the_bottom_follows_again() {
    let (mut editor, id) = following(60);
    let window = editor.window().id();
    editor.scroll_view(window, -10);
    assert!(!at_end(&editor, window));
    editor.scroll_view(window, 4);
    assert!(!at_end(&editor, window), "not at the bottom yet");
    editor.scroll_view(window, 30);
    assert!(at_end(&editor, window), "the wheel");
    let _ = editor.scroll_lines(-10);
    assert!(!at_end(&editor, window));
    let _ = editor.scroll_lines(10);
    assert!(at_end(&editor, window), "C-v");
    add(&mut editor, id, "more\n");
    assert!(at_end(&editor, window));
    // Another buffer's cursor stays where it is in view.
    let text = Buffer::from_text("notes", &"line\n".repeat(60));
    let notes = editor.add_buffer(text);
    editor.show(notes);
    editor.scroll_view(window, 100);
    assert!(shows_end(&editor, window) && !at_end(&editor, window));
}

#[test]
fn a_region_stays_where_it_is_as_the_wheel_reaches_the_bottom() {
    let (mut editor, _) = following(60);
    let window = editor.window().id();
    editor.scroll_view(window, -10);
    let (shown, _) = editor.current_mut();
    shown.selections.push_marks();
    shown.selections.set_mark_active(MarkActive::Active);
    editor.scroll_view(window, 30);
    assert!(shows_end(&editor, window) && !at_end(&editor, window));
}

#[test]
fn a_cursor_in_a_row_drawn_again_stays_in_it() {
    let (mut editor, id) = following(3);
    let window = editor.window().id();
    let last = "row 0\nrow 1\n".len();
    for at in [last, last + 2] {
        editor.current_mut().0.selections = SelectionSet::new(at);
        let end = len(&editor, id);
        editor.replace_native_rows(id, last..end, "row 2 says more\nand more\n", &[]);
        assert!(cursor(&editor, window) <= at, "from {at}");
        assert!(cursor(&editor, window) >= last, "from {at}");
    }
}

#[test]
fn a_reader_in_a_row_that_grows_stays_where_it_reads() {
    let (mut editor, id) = following(0);
    let window = editor.window().id();
    editor.set_wraps(id, true);
    let long = "word ".repeat(1000);
    add(&mut editor, id, &format!("{long}\n"));
    crate::editor::wrap_tests::settle(&mut editor);
    editor.scroll_view(window, -30);
    let (left, before) = (cursor(&editor, window), top(&editor, window));
    assert!(before > 0 && left > 0);
    for grown in 1..4 {
        let end = len(&editor, id);
        let text = format!("{long}{}\n", "more ".repeat(100 * grown));
        editor.replace_native_rows(id, 0..end, &text, &[]);
        crate::editor::wrap_tests::settle(&mut editor);
        assert_eq!(
            (cursor(&editor, window), top(&editor, window)),
            (left, before)
        );
    }
    // Of the same text, only the runs are put in again.
    let end = len(&editor, id);
    let same = editor.buffer(id).unwrap().text().to_string();
    let runs = [(4, Some(crate::syntax::Highlight::SessionAlert))];
    editor.replace_native_rows(id, 0..end, &same, &runs);
    let highlights = editor.buffer(id).unwrap().highlights().unwrap();
    assert_eq!(
        highlights.at(1),
        Some(crate::syntax::Highlight::SessionAlert)
    );
}

#[test]
fn a_wrap_report_beneath_a_prompt_does_not_take_the_view_back() {
    let (mut editor, id) = following(60);
    let window = editor.window().id();
    editor.set_wraps(id, true);
    crate::editor::wrap_tests::settle(&mut editor);
    editor
        .open_prompt("Go to line: ", Kind::Number { history: None })
        .unwrap();
    editor.scroll_view(window, -20);
    let before = top(&editor, window);
    add(&mut editor, id, &"word ".repeat(400));
    let jobs = editor.take_wrap_jobs();
    assert!(!jobs.is_empty(), "past the budget");
    for job in jobs {
        if let Some(report) = job.run() {
            editor.take_wrap_report(report);
        }
    }
    assert_eq!(top(&editor, window), before);
    editor.quit();
    editor.follow_cursor();
    assert!(at_end(&editor, window));
}

#[test]
fn an_isearch_given_up_goes_back_to_the_end() {
    let (mut editor, id) = following(60);
    let window = editor.window().id();
    assert!(editor.isearch_begin(true, false));
    editor.current_mut().0.selections.move_primary(3);
    add(&mut editor, id, &"row\n".repeat(60));
    editor.quit();
    assert!(editor.isearch().is_none());
    assert!(at_end(&editor, window));
}

#[test]
fn beneath_a_prompt_the_view_follows_unless_the_wheel_left_the_end() {
    let (mut editor, id) = following(60);
    let window = editor.window().id();
    editor.set_preedit(Some(Preedit::new("か".into(), None)));
    add(&mut editor, id, "one\n");
    assert!(at_end(&editor, window));
    editor.scroll_view(window, -20);
    let before = top(&editor, window);
    add(&mut editor, id, "two\n");
    assert_eq!(top(&editor, window), before, "the wheel left the end");
    editor.set_preedit(None);
    assert!(at_end(&editor, window), "its cursor never left it");
    // A prompt too, whose own window is selected.
    editor
        .open_prompt("Go to line: ", Kind::Number { history: None })
        .unwrap();
    add(&mut editor, id, "three\n");
    assert!(at_end(&editor, window));
    editor.scroll_view(window, -20);
    let before = top(&editor, window);
    add(&mut editor, id, "four\n");
    assert_eq!(top(&editor, window), before);
    editor.quit();
    editor.follow_cursor();
    assert!(at_end(&editor, window));
}

#[test]
fn the_preview_of_a_transcript_shows_the_end_it_will_show() {
    let (mut editor, id) = following(60);
    let other = editor.add_buffer(Buffer::from_text("notes", "x\n"));
    editor.show(other);
    add(&mut editor, id, &"row\n".repeat(60));
    let end = len(&editor, id);
    let style = CompletionStyle {
        given_order: true,
        takes_input: true,
        preview: true,
    };
    let names = vec![Candidate::new("*claude: oso*", "", "")];
    editor
        .open_completion_as("Switch to buffer:", "buffer", names.into(), style)
        .unwrap();
    assert!(matches!(editor.preview(), Some(Preview::Buffer { cursor, .. }) if cursor == end));
    editor.quit();
    // From its composer, where the window has left it higher up.
    editor.show(id);
    let window = editor.window().id();
    let composer = composer(&mut editor, id);
    editor.scroll_view(window, -20);
    let left = cursor(&editor, window);
    editor.select_window(composer);
    let names = vec![Candidate::new("*claude: oso*", "", "")];
    editor
        .open_completion_as("Switch to buffer:", "buffer", names.into(), style)
        .unwrap();
    assert!(matches!(editor.preview(), Some(Preview::Buffer { cursor, .. }) if cursor == left));
}

#[test]
fn a_zoom_in_and_out_keeps_the_end() {
    let (mut editor, id) = following(60);
    let window = editor.window().id();
    let lone = editor.lone_window_tree();
    let kept = editor.swap_window_tree(lone).unwrap();
    assert!(at_end(&editor, editor.window().id()));
    add(&mut editor, id, &"row\n".repeat(60));
    assert!(at_end(&editor, editor.window().id()));
    editor.swap_window_tree(kept).unwrap();
    assert!(at_end(&editor, window));
}

#[test]
fn a_mark_and_the_other_cursors_stay_as_a_window_comes_back_to_the_end() {
    let (mut editor, id) = following(60);
    let end = len(&editor, id);
    let shown = SelectionSet::from_parts(vec![Selection::at(4), Selection::at(end)], 1);
    editor.current_mut().0.selections = shown.unwrap();
    editor.current_mut().0.selections.push_marks();
    let other = editor.add_buffer(Buffer::from_text("notes", "x\n"));
    editor.show(other);
    add(&mut editor, id, &"row\n".repeat(60));
    editor.show(id);
    let set = &editor.window().selections;
    assert_eq!(set.primary().cursor, len(&editor, id));
    assert_eq!(set.primary().mark, Some(end));
    assert_eq!(set.len(), 2);
}

/// A timing to read: the UI thread's share of a row added to a transcript in three windows,
/// its composer selected.
#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(
    clippy::disallowed_macros,
    reason = "the timing is read from what it prints"
)]
fn a_row_added_costs_as_much_at_10000_rows_as_at_100() {
    for (rows, wraps) in [
        (100, false),
        (10_000, false),
        (100_000, false),
        (100, true),
        (10_000, true),
    ] {
        let (mut editor, id) = following(0);
        let row =
            "a row of a transcript, long enough to wrap once at the width of eighty columns\n";
        add(&mut editor, id, &row.repeat(rows));
        let composer = composer(&mut editor, id);
        editor.split_window(Way::Across, false).unwrap();
        editor.split_window(Way::Down, false).unwrap();
        editor.set_wraps(id, wraps);
        if wraps {
            crate::editor::wrap_tests::settle(&mut editor);
        }
        editor.select_window(composer);
        let mut took: Vec<_> = (0..1000)
            .map(|row| {
                editor.refill_wraps();
                let began = std::time::Instant::now();
                add(&mut editor, id, &format!("row {row}\n"));
                began.elapsed()
            })
            .collect();
        took.sort_unstable();
        println!(
            "{rows} rows, wraps {wraps}: p50 {:?} p99 {:?}",
            took[500], took[990]
        );
    }
}
