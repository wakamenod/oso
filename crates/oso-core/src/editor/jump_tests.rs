//! Tests of jumps on the editor (ADR 0116): what each kind finds in view and in what order,
//! the labels and their keys, where a jump goes in which window, how it ends, and what it
//! keeps from a buffer that may hold secrets.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::test_buffers::secret_buffer;
use super::{Chose, Editor, JumpCells, Quit, Status};
use crate::buffer::{Buffer, Replacement};
use crate::frame::{FrameSize, Way};
use crate::jump::{Keys, Kind, MAX_LABELLED, Stage};
use crate::minibuffer::Kind as PromptKind;
use crate::selection::{MarkActive, Selection, SelectionSet};
use crate::window::Screen;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 40,
    });
    editor
}

fn cursor(editor: &Editor) -> usize {
    editor.window().selections.primary().cursor
}

fn line(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

/// The candidates' starts, in label order.
fn targets(editor: &Editor) -> Vec<usize> {
    let jump = editor.jump().unwrap();
    jump.candidates.iter().map(|c| c.range.start).collect()
}

fn jump_for(editor: &mut Editor, string: &str) {
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add(string);
    editor.jump_show_labels();
}

fn cells(editor: &Editor) -> JumpCells {
    let mut screen = Screen::default();
    let id = editor.window().id();
    editor.screen_of(id, &mut screen);
    let mut cells = JumpCells::default();
    editor.jump_cells(id, &screen, &mut cells);
    cells
}

#[test]
fn a_string_is_read_then_its_matches_are_labelled_nearest_first() {
    let mut editor = editor("ab ab\nxx ab\nab\n");
    editor.current_mut().0.selections = SelectionSet::new(9);
    assert!(editor.jump_begin(Kind::Char, false));
    assert_eq!(line(&editor), "Jump: ");
    editor.jump_add("ab");
    let jump = editor.jump().unwrap();
    assert_eq!(
        (jump.stage(), jump.count(), jump.labelled()),
        (Stage::Reading, 4, 0)
    );
    assert_eq!(line(&editor), "Jump: ab");
    assert_eq!(cells(&editor).matches.len(), 4, "highlighted while read");
    editor.jump_show_labels();
    assert_eq!(editor.jump().unwrap().labelled(), 4);
    // The cursor's row first, then a row away either way by columns from the cursor's.
    assert_eq!(targets(&editor), [9, 3, 0, 12]);
    assert_eq!(editor.jump_choose('s'), Chose::Jumped);
    assert!(editor.jump().is_none());
    assert_eq!(cursor(&editor), 3);
    let primary = editor.window().selections.primary();
    assert_eq!(primary.mark, Some(9), "where it left, with no message");
    assert_eq!(line(&editor), "");
}

#[test]
fn delete_takes_a_character_back_and_the_string_has_a_limit() {
    let mut editor = editor("abc abd\n");
    editor.jump_begin(Kind::Char, false);
    editor.jump_add("abc");
    assert_eq!(editor.jump().unwrap().count(), 1);
    assert!(editor.jump_delete());
    assert_eq!(editor.jump().unwrap().count(), 2);
    editor.jump_add(&"x".repeat(100));
    assert_eq!(line(&editor).chars().count(), "Jump: ".len() + 64);
    let mut editor = self::editor("a");
    editor.jump_begin(Kind::Char, false);
    assert!(!editor.jump_delete());
}

#[test]
fn a_label_of_two_keys_narrows_at_its_first() {
    let mut editor = editor("a a a\n");
    editor.set_jump_keys(Keys::new("xy").unwrap());
    jump_for(&mut editor, "a");
    // Three candidates on two keys: `x`, then `yx` and `yy`.
    assert_eq!(targets(&editor), [0, 2, 4]);
    assert!(editor.jump().unwrap().takes('y'));
    assert!(!editor.jump().unwrap().takes('z'));
    assert_eq!(editor.jump_choose('y'), Chose::Narrowed);
    let shown = cells(&editor);
    assert_eq!(shown.labels.len(), 2, "only those that start with `y`");
    assert_eq!(shown.text, "xy", "what is left of each");
    assert_eq!(editor.jump_choose('y'), Chose::Jumped);
    assert_eq!(cursor(&editor), 4);
}

#[test]
fn a_key_no_label_goes_on_with_ends_the_jump_with_nothing_moved() {
    let mut editor = editor("a a\n");
    jump_for(&mut editor, "a");
    assert_eq!(editor.jump_choose('z'), Chose::NoSuch);
    assert!(editor.jump().is_none());
    assert_eq!(cursor(&editor), 0);
    assert_eq!(line(&editor), "No such candidate");
}

#[test]
fn del_and_ret_at_the_labels_end_the_jump_as_any_other_key() {
    let mut editor = editor("a a\n");
    jump_for(&mut editor, "a");
    assert!(!editor.jump_delete());
    assert!(editor.jump().is_none());
    assert_eq!(line(&editor), "No such candidate");

    jump_for(&mut editor, "a");
    editor.jump_show_labels();
    assert!(editor.jump().is_none());
    assert_eq!(line(&editor), "No such candidate");
    assert_eq!(cursor(&editor), 0);
    assert!(!editor.jump_refuse(), "none under way");
}

#[test]
fn nothing_to_label_ends_the_jump() {
    let mut editor = editor("abc\n");
    jump_for(&mut editor, "zz");
    assert!(editor.jump().is_none());
    assert_eq!(line(&editor), "No candidates");
}

#[test]
fn a_string_not_begun_waits_for_a_key() {
    let mut editor = editor("abc\n");
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_show_labels();
    assert_eq!(editor.jump().map(|jump| jump.stage()), Some(Stage::Reading));
    editor.jump_add("b");
    editor.jump_show_labels();
    assert_eq!(editor.jump().map(|jump| jump.stage()), Some(Stage::Labels));
}

#[test]
fn a_line_jump_labels_each_line_in_view_at_once() {
    let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
    let mut editor = editor(&text);
    let row = |n: usize| text.lines().take(n).map(|l| l.len() + 1).sum::<usize>();
    editor.current_mut().0.selections = SelectionSet::new(row(3) + 2);
    assert!(editor.jump_begin(Kind::LineStart, false));
    let jump = editor.jump().unwrap();
    assert_eq!(jump.stage(), Stage::Labels);
    assert_eq!(jump.count(), 10, "the rows in view");
    assert_eq!(&targets(&editor)[..3], [row(3), row(2), row(4)]);
    assert_eq!(line(&editor), "Jump to a line");
    editor.jump_end();
    assert!(editor.jump_begin(Kind::LineEnd, false));
    assert_eq!(targets(&editor)[0], row(4) - 1);
}

#[test]
fn a_label_is_cut_where_the_next_begins_and_shows_after_an_empty_lines_end() {
    let mut editor = editor("aaa\n\n");
    editor.set_jump_keys(Keys::new("xy").unwrap());
    jump_for(&mut editor, "a");
    // `x`, `yx`, `yy`: the second is cut at the third.
    let shown = cells(&editor);
    let at: Vec<_> = (shown.labels.iter())
        .map(|label| {
            (
                label.cells.column,
                label.cells.columns,
                &shown.text[label.keys.clone()],
            )
        })
        .collect();
    let left = shown.labels[0].cells.column;
    assert_eq!(
        at,
        [(left, 1, "x"), (left + 1, 1, "y"), (left + 2, 2, "yy")]
    );
    editor.jump_end();
    editor.current_mut().0.selections = SelectionSet::new(4);
    assert!(editor.jump_begin(Kind::LineEnd, false));
    let shown = cells(&editor);
    let first = shown.labels[0].cells;
    let empty = shown
        .labels
        .iter()
        .find(|label| label.cells.row == first.row + 1);
    assert_eq!(empty.unwrap().cells.column, left);
    assert_eq!(empty.unwrap().cells.columns, 1, "the cell after the text");
}

#[test]
fn every_window_is_labelled_and_a_jump_into_another_selects_it() {
    let mut editor = editor("aa\n");
    let first = editor.window().id();
    let other = editor.split_window(Way::Down, false).unwrap();
    jump_for(&mut editor, "a");
    let jump = editor.jump().unwrap();
    assert_eq!(jump.count(), 4);
    let windows: Vec<_> = jump.candidates.iter().map(|c| c.window).collect();
    assert_eq!(
        windows,
        [first, first, other, other],
        "the selected window first"
    );
    assert_eq!(editor.jump_choose('d'), Chose::Jumped);
    assert_eq!(editor.window().id(), other);
    assert_eq!(cursor(&editor), 0);
    editor.select_window(first);
    // `C-u` keeps to the selected window.
    assert!(editor.jump_begin(Kind::Char, true));
    editor.jump_add("a");
    assert_eq!(editor.jump().unwrap().count(), 2);
}

#[test]
fn c_g_ends_a_jump_before_anything_else() {
    let mut editor = editor("a a\n");
    editor
        .current_mut()
        .0
        .selections
        .set_mark_active(MarkActive::Active);
    jump_for(&mut editor, "a");
    assert_eq!(editor.quit(), Quit::Jump);
    assert!(editor.jump().is_none());
    assert!(editor.window().selections.mark_active().is_active());
    assert_eq!(editor.quit(), Quit::Region);
}

#[test]
fn a_jump_does_not_work_in_a_prompt_and_a_prompt_ends_one() {
    let mut editor = editor("a\n");
    let number = PromptKind::Number { history: None };
    editor.open_prompt("Go to line: ", number).unwrap();
    assert!(!editor.jump_begin(Kind::Char, false));
    let mut editor = self::editor("a\n");
    jump_for(&mut editor, "a");
    editor.open_prompt("Go to line: ", number).unwrap();
    assert!(editor.jump().is_none(), "at once");
}

#[test]
fn a_shown_region_is_extended_and_the_other_cursors_stay() {
    let mut editor = editor("a b a b a\n");
    let selections = vec![Selection::at(0), Selection::at(4)];
    editor.current_mut().0.selections = SelectionSet::from_parts(selections, 1).unwrap();
    editor.current_mut().0.selections.push_primary_mark(4);
    editor
        .current_mut()
        .0
        .selections
        .set_mark_active(MarkActive::Active);
    jump_for(&mut editor, "a");
    let label = targets(&editor).iter().position(|&at| at == 8).unwrap();
    let key = ['a', 's', 'd'][label];
    assert_eq!(editor.jump_choose(key), Chose::Jumped);
    let selections = &editor.window().selections;
    assert_eq!(selections.primary().cursor, 8);
    assert_eq!(
        selections.primary().mark,
        Some(4),
        "the region's mark, not where it left"
    );
    assert!(selections.mark_active().is_active());
    assert!(
        selections.iter().any(|s| s.cursor == 0),
        "the other cursor stays"
    );
}

#[test]
fn a_change_under_the_labels_ends_the_jump_saying_so_and_takes_the_next_key() {
    let mut editor = editor("a a\n");
    jump_for(&mut editor, "a");
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..1,
        text: "b",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    assert!(editor.isearch_follow_text());
    assert_eq!(line(&editor), "Jump ended: the text changed");
    // The person did not end it: the key meant for a label goes nowhere, and says why.
    let jump = editor.jump().unwrap();
    assert_eq!((jump.labelled(), jump.takes('a')), (0, false));
    let drawn = cells(&editor);
    assert!(
        drawn.labels.is_empty() && drawn.matches.is_empty(),
        "it draws nothing"
    );
    assert!(!editor.isearch_follow_text(), "nothing more to follow");
    editor.clear_echo();
    let before = cursor(&editor);
    assert_eq!(editor.jump_choose('a'), Chose::NoSuch);
    assert!(editor.jump().is_none());
    assert_eq!(cursor(&editor), before, "nothing moved");
    assert_eq!(line(&editor), "Jump ended: the text changed");
}

#[test]
fn a_line_jumps_digits_go_to_goto_line_only_before_a_label_is_begun() {
    let mut editor = editor(&"x\n".repeat(20));
    editor.set_jump_keys(Keys::new("12").unwrap());
    assert!(editor.jump_begin(Kind::LineStart, false));
    let jump = editor.jump().unwrap();
    assert!(!jump.gives_goto_line('1'), "a label's key");
    assert!(jump.gives_goto_line('7'));
    assert!(!jump.gives_goto_line('x'));
    assert_eq!(editor.jump_choose('1'), Chose::Narrowed);
    assert!(
        !editor.jump().unwrap().gives_goto_line('7'),
        "a label is begun"
    );
}

#[test]
fn a_scroll_finds_the_matches_again_while_reading_and_ends_the_labels() {
    let text = "a\n".repeat(100);
    let mut editor = editor(&text);
    let id = editor.tiled().0.id();
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add("a");
    assert_eq!(targets(&editor)[0], 0);
    assert!(editor.scroll_view(id, 20));
    assert!(editor.isearch_follow_text(), "found again");
    let top = editor.top_row() * 2;
    assert!(targets(&editor).iter().all(|&at| at >= top), "in view");
    assert!(!editor.isearch_follow_text(), "nothing moved since");

    editor.jump_show_labels();
    assert_eq!(editor.jump().unwrap().stage(), Stage::Labels);
    assert!(editor.scroll_view(id, -5));
    assert!(editor.isearch_follow_text());
    assert_eq!(line(&editor), "Jump ended: the view changed");
    assert_eq!(editor.jump_choose('a'), Chose::NoSuch);
    assert!(editor.jump().is_none());
}

#[test]
fn a_wrapped_lines_rows_are_matched_as_one_and_labels_leave_the_mark() {
    let mut editor = editor(&format!("{}\n", "a".repeat(25)));
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 11,
    });
    let id = editor.window().buffer();
    assert!(editor.set_wraps(id, true));
    super::wrap_tests::settle(&mut editor);
    jump_for(&mut editor, "aa");
    let starts = targets(&editor);
    assert_eq!(
        starts.len(),
        12,
        "as isearch: none overlaps another across rows"
    );
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..12).map(|at| at * 2).collect::<Vec<_>>());
    let mut screen = Screen::default();
    editor.screen_of(editor.window().id(), &mut screen);
    let width = screen.drawn(&screen.rows()[0]).len();
    assert!(width < 11, "the row draws the mark");
    for label in &cells(&editor).labels {
        assert!(
            label.cells.column + label.cells.columns <= width,
            "{label:?}"
        );
    }
}

#[test]
fn past_the_most_labelled_the_farthest_have_none_and_the_line_says_so() {
    let text = format!("{}\n", "a".repeat(99)).repeat(60);
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", &text));
    editor.set_frame_size(FrameSize {
        rows: 62,
        columns: 100,
    });
    jump_for(&mut editor, "a");
    let jump = editor.jump().unwrap();
    assert_eq!((jump.count(), jump.labelled()), (5940, MAX_LABELLED));
    assert_eq!(line(&editor), "Jump: a  [4096 of 5940 labelled]");
    assert!(
        targets(&editor)
            .iter()
            .all(|&at| at < 4096 / 99 * 100 + 100),
        "the nearest"
    );
    // 41 rows of 99, then 37 of the row the cut falls in: its first, in the text's order.
    let mut cut: Vec<_> = targets(&editor)
        .into_iter()
        .filter(|&at| at >= 4100)
        .collect();
    cut.sort_unstable();
    assert_eq!(cut, (4100..4137).collect::<Vec<_>>());
}

#[test]
fn avy_isearch_ends_the_search_at_the_match_chosen() {
    let mut editor = editor("foo bar foo\n");
    assert!(!editor.jump_begin(Kind::Isearch, false), "only in a search");
    editor.isearch_begin(false, false);
    editor.isearch_add("foo");
    assert!(editor.jump_begin(Kind::Isearch, false));
    assert_eq!(targets(&editor), [0, 8]);
    assert_eq!(editor.jump_choose('s'), Chose::Jumped);
    assert!(editor.isearch().is_none());
    assert_eq!(cursor(&editor), 8);
    assert_eq!(
        editor.window().selections.primary().mark,
        Some(0),
        "where it began"
    );
}

#[test]
fn avy_isearch_labels_a_regexp_search_s_matches() {
    let mut editor = editor("fo bar foo fx\n");
    editor.isearch_begin(false, true);
    editor.isearch_add("fo+");
    assert!(editor.jump_begin(Kind::Isearch, false));
    assert_eq!(targets(&editor), [0, 7]);
}

#[test]
fn avy_isearch_with_a_regexp_on_a_long_line_reads_around_the_view() {
    // One line of 4 MiB, the cursor in its middle.
    let text = "foo bar ".repeat(512 * 1024);
    let mut editor = editor(&text);
    let middle = text.len() / 2;
    editor.window_mut(0).0.selections = SelectionSet::new(middle);
    editor.follow_cursor();
    editor.isearch_begin(false, true);
    editor.isearch_add("fo+ ");
    // A line this long is the worker's for the search itself.
    while let Some(job) = editor.take_isearch_job() {
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        for report in reports {
            editor.isearch_reported(report);
        }
    }
    let at = cursor(&editor);
    assert_eq!(at, middle + 4);
    let started = std::time::Instant::now();
    assert!(editor.jump_begin(Kind::Isearch, false));
    let took = started.elapsed();
    let found = targets(&editor);
    assert!(!found.is_empty());
    // Only what the view shows is read, a few rows of 40 columns around the cursor.
    assert!(
        found.iter().all(|&start| start.abs_diff(at) < 4096),
        "{found:?}"
    );
    assert!(took.as_millis() < 50, "{took:?}");
}

#[test]
fn around_a_secret_the_jump_says_so_and_debug_holds_no_string() {
    let mut editor = Editor::with_buffer(secret_buffer());
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 40,
    });
    editor.jump_begin(Kind::Char, false);
    editor.jump_add("hunter2");
    let jump = editor.jump().unwrap();
    assert!(jump.around_secret());
    assert!(!format!("{jump:?}").contains("hunter2"));
}

#[test]
fn a_jump_is_around_a_secret_as_long_as_what_it_labels_or_follows_was() {
    // `avy-isearch` labels what a search around a secret found.
    let mut searched = editor("foo bar foo\n");
    searched.isearch_begin(false, false);
    searched.isearch_add("foo");
    searched.isearch.as_mut().unwrap().note_secret();
    assert!(searched.jump_begin(Kind::Isearch, false));
    assert!(searched.jump().unwrap().around_secret());
    assert!(searched.around_secret());

    // A secret buffer shown while a jump reads marks it, and the editor with it.
    let mut reading = editor("foo bar foo\n");
    assert!(reading.jump_begin(Kind::Char, false));
    assert!(!reading.around_secret());
    let secret = reading.add_buffer(secret_buffer());
    reading.show(secret);
    assert!(reading.jump().is_some_and(|jump| jump.around_secret()));
    assert!(reading.around_secret());
}

#[test]
fn rows_are_looked_at_from_the_cursors_outward_so_a_budget_leaves_out_the_farthest() {
    let order = |at, count| super::jump::outward(at, count).collect::<Vec<_>>();
    assert_eq!(order(2, 5), [2, 3, 1, 4, 0]);
    assert_eq!(order(0, 3), [0, 1, 2]);
    assert_eq!(
        order(4, 3),
        [2, 1, 0],
        "a cursor below the rows starts at the last"
    );
    assert_eq!(order(0, 0), Vec::<usize>::new());
}
