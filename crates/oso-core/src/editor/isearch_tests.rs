//! Tests of isearch on the editor (ADR 0082): where it goes, its count from a worker run in
//! place, its steps back, how it ends, and what it keeps from a buffer that may hold secrets.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::isearch::Tally;
use super::test_buffers::secret_buffer;
use super::{Editor, Quit, Status};
use crate::buffer::{Buffer, Replacement};
use crate::frame::CellRect;
use crate::search::{KEY_BUDGET, MAX_QUERY_CHARS};
use crate::selection::{Selection, SelectionSet};
use crate::window::ViewSize;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_view_size(ViewSize::new(10, 80));
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

#[test]
fn typing_goes_to_the_end_of_the_first_match_and_counts() {
    let mut editor = editor("one foo two foo three foo");
    editor.isearch_begin(false, false);
    assert_eq!(line(&editor), "I-search: ");
    editor.isearch_add("fo");
    assert_eq!(cursor(&editor), 6);
    assert_eq!(line(&editor), "I-search: fo [?/…]");
    editor.isearch_add("o");
    drain(&mut editor);
    assert_eq!(cursor(&editor), 7);
    assert_eq!(line(&editor), "I-search: foo [1/3]");
}

#[test]
fn smart_case_holds_until_m_c() {
    let mut editor = editor("Foo foo");
    search(&mut editor, "foo");
    assert_eq!(cursor(&editor), 3, "folded, so the first");
    editor.isearch_delete();
    editor.isearch_delete();
    editor.isearch_delete();
    search(&mut editor, "Foo");
    let mut editor = self::editor("foo Foo");
    search(&mut editor, "Foo");
    assert_eq!(cursor(&editor), 7, "an uppercase letter, so sensitive");
    editor.isearch_toggle_case();
    drain(&mut editor);
    assert_eq!(cursor(&editor), 7, "the match still matches");
    assert_eq!(line(&editor), "Case-insensitive I-search: Foo [2/2]");
    assert!(!editor.isearch().unwrap().case_sensitive());
}

#[test]
fn repeats_step_the_count_fail_at_the_end_and_wrap() {
    let mut editor = editor("ab ab ab");
    search(&mut editor, "ab");
    assert_eq!(line(&editor), "I-search: ab [1/3]");
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 5);
    assert_eq!(line(&editor), "I-search: ab [2/3]");
    editor.isearch_repeat(false);
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 8, "stays at the last match");
    assert_eq!(line(&editor), "Failing I-search: ab [3/3]");
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 2);
    assert_eq!(line(&editor), "Wrapped I-search: ab [1/3]");
    assert_eq!(editor.take_isearch_job().map(|job| job.generation()), None);
}

#[test]
fn backward_goes_to_the_start_and_a_turn_finds_the_one_before() {
    let mut editor = editor("ab ab ab");
    editor.isearch_begin(true, false);
    editor.isearch_add("ab");
    drain(&mut editor);
    // From the start of the buffer, nothing is before.
    assert!(editor.isearch().unwrap().failing());
    editor.isearch_repeat(true);
    drain(&mut editor);
    assert_eq!(cursor(&editor), 6);
    assert_eq!(line(&editor), "Wrapped I-search backward: ab [3/3]");
    editor.isearch_repeat(true);
    assert_eq!(cursor(&editor), 3);
    assert_eq!(line(&editor), "Wrapped I-search backward: ab [2/3]");
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 8, "the match after this one, from its end");
}

#[test]
fn typing_backward_keeps_a_match_that_still_matches() {
    let mut editor = editor("foo x foo y");
    editor.window_mut(0).0.selections = SelectionSet::new(11);
    editor.isearch_begin(true, false);
    editor.isearch_add("f");
    assert_eq!(cursor(&editor), 6);
    editor.isearch_add("oo");
    assert_eq!(cursor(&editor), 6);
}

#[test]
fn delete_goes_back_a_step_with_its_place() {
    let mut editor = editor("ab ab ac");
    search(&mut editor, "a");
    editor.isearch_add("b");
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 5);
    assert!(editor.isearch_delete());
    assert_eq!(cursor(&editor), 2, "the repeat undone");
    assert!(editor.isearch_delete());
    assert_eq!(editor.isearch().unwrap().string(), "a");
    assert!(editor.isearch_delete());
    assert_eq!(editor.isearch().unwrap().string(), "");
    assert_eq!(cursor(&editor), 0);
    assert!(!editor.isearch_delete());
}

#[test]
fn c_g_takes_the_failing_part_then_ends_where_it_began() {
    let mut editor = editor(&"word\n".repeat(40));
    editor.window_mut(0).0.selections = SelectionSet::new(3);
    let scroll = editor.window().scroll();
    search(&mut editor, "word");
    editor.isearch_repeat(false);
    editor.isearch_add("xy");
    drain(&mut editor);
    assert!(editor.isearch().unwrap().failing());
    assert_eq!(editor.quit(), Quit::SearchStep);
    let state = editor.isearch().unwrap();
    assert_eq!(state.string(), "word");
    assert!(!state.failing());
    assert_eq!(cursor(&editor), 14);
    assert_eq!(editor.quit(), Quit::Search);
    assert!(editor.isearch().is_none());
    assert_eq!(cursor(&editor), 3);
    assert_eq!(editor.window().scroll(), scroll);
    assert_eq!(editor.quit(), Quit::Nothing);
}

#[test]
fn c_g_hides_a_region_before_it_quits_the_search() {
    let mut editor = editor("a b a b");
    editor.windows[0].selections.push_marks();
    editor.windows[0]
        .selections
        .set_mark_active(crate::selection::MarkActive::Active);
    search(&mut editor, "b");
    assert_eq!(editor.quit(), Quit::Region);
    assert_eq!(editor.quit(), Quit::Search);
}

#[test]
fn exit_pushes_the_start_as_the_mark_and_keeps_the_string() {
    let mut editor = editor("x needle y needle");
    search(&mut editor, "needle");
    assert!(editor.isearch_exit());
    let primary = *editor.window().selections.primary();
    assert_eq!(primary.cursor, 8);
    assert_eq!(primary.mark, Some(0));
    assert_eq!(line(&editor), "Mark saved where search started");
    // `C-s C-s` takes it up again.
    editor.window_mut(0).0.selections = SelectionSet::new(0);
    editor.isearch_begin(false, false);
    editor.isearch_repeat(false);
    drain(&mut editor);
    assert_eq!(cursor(&editor), 8);
    assert_eq!(editor.isearch().unwrap().string(), "needle");
}

#[test]
fn an_exit_where_it_began_sets_no_mark() {
    let mut editor = editor("abc");
    editor.isearch_begin(false, false);
    assert!(editor.isearch_exit());
    assert_eq!(editor.window().selections.primary().mark, None);
    assert!(!editor.isearch_exit());
}

#[test]
fn the_history_goes_back_and_forth() {
    let mut editor = editor("one two three");
    for word in ["one", "two"] {
        search(&mut editor, word);
        editor.isearch_exit();
    }
    editor.isearch_begin(false, false);
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "two");
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "one");
    editor.isearch_history(false);
    assert_eq!(editor.isearch().unwrap().string(), "two");
}

#[test]
fn a_far_match_is_the_workers() {
    let text = format!("{}needle", "x".repeat(KEY_BUDGET * 2));
    let mut editor = editor(&text);
    editor.isearch_begin(false, false);
    editor.isearch_add("needle");
    assert!(editor.isearch().unwrap().searching());
    assert_eq!(cursor(&editor), 0);
    assert_eq!(line(&editor), "I-search: needle [searching…]");
    // A repeat waits for it.
    editor.isearch_repeat(false);
    drain(&mut editor);
    assert_eq!(cursor(&editor), text.len());
    assert_eq!(line(&editor), "I-search: needle [1/1]");
}

#[test]
fn c_g_stops_a_running_search() {
    let text = format!("ab{}abc", "x".repeat(KEY_BUDGET * 2));
    let mut editor = editor(&text);
    search(&mut editor, "ab");
    editor.isearch_add("c");
    assert!(editor.isearch().unwrap().searching());
    let job = editor.take_isearch_job().unwrap();
    assert_eq!(editor.quit(), Quit::SearchStep);
    assert!(job.superseded());
    assert_eq!(editor.isearch().unwrap().string(), "ab");
    assert_eq!(cursor(&editor), 2);
}

#[test]
fn a_stale_report_is_dropped() {
    let mut editor = editor("ab ab");
    editor.isearch_begin(false, false);
    editor.isearch_add("a");
    let old = editor.take_isearch_job().unwrap();
    editor.isearch_add("b");
    let mut reports = Vec::new();
    old.run(|report| reports.push(report));
    for report in reports {
        assert!(!editor.isearch_reported(report));
    }
    assert_eq!(
        editor.isearch().unwrap().tally(),
        Some(Tally::Counting { moves: 0 })
    );
}

#[test]
fn moves_while_counting_count_from_the_worker_s_answer() {
    let mut editor = editor("a a a a");
    search(&mut editor, "a");
    editor.isearch_delete();
    editor.isearch_add("a");
    let job = editor.take_isearch_job().unwrap();
    editor.isearch_repeat(false);
    editor.isearch_repeat(false);
    let mut reports = Vec::new();
    job.run(|report| reports.push(report));
    for report in reports {
        editor.isearch_reported(report);
    }
    assert_eq!(line(&editor), "I-search: a [3/4]");
}

#[test]
fn a_change_to_the_text_searches_again() {
    let mut editor = editor("xx foo");
    search(&mut editor, "foo");
    assert_eq!(cursor(&editor), 6);
    let id = editor.window().buffer();
    let edit = [Replacement {
        range: 0..0,
        text: "foo ",
    }];
    editor.apply_edits(id, &edit).unwrap();
    assert!(editor.isearch_follow_text());
    drain(&mut editor);
    assert_eq!(line(&editor), "I-search: foo [2/2]");
    assert!(!editor.isearch_follow_text());
}

#[test]
fn only_the_primary_cursor_moves() {
    let mut editor = editor("a b c b");
    let set = SelectionSet::from_parts(vec![Selection::at(0), Selection::at(4)], 0).unwrap();
    editor.window_mut(0).0.selections = set;
    search(&mut editor, "c");
    let cursors: Vec<usize> = editor
        .window()
        .selections
        .iter()
        .map(|s| s.cursor)
        .collect();
    assert_eq!(cursors, [4, 5]);
    assert_eq!(cursor(&editor), 5);
    editor.quit();
    let cursors: Vec<usize> = editor
        .window()
        .selections
        .iter()
        .map(|s| s.cursor)
        .collect();
    assert_eq!(cursors, [0, 4]);
}

#[test]
fn yanks_add_the_word_after_the_match_and_the_newest_kill() {
    let mut editor = editor("say hello world, hello there");
    search(&mut editor, "hel");
    editor.isearch_yank_word();
    assert_eq!(editor.isearch().unwrap().string(), "hello");
    editor.isearch_yank_word();
    assert_eq!(editor.isearch().unwrap().string(), "hello ");
    editor.isearch_yank_word();
    assert_eq!(editor.isearch().unwrap().string(), "hello world");
    editor.isearch_exit();
    editor.kills.ring.push(
        vec![crate::kill::Piece::from(crate::rope::Rope::from("there"))],
        false,
    );
    editor.isearch_begin(false, false);
    editor.isearch_yank_kill();
    assert_eq!(editor.isearch().unwrap().string(), "there");
    assert_eq!(cursor(&editor), 28);
}

#[test]
fn the_matches_in_view_are_cells_and_the_current_one_apart() {
    let mut editor = editor("ab\nxab ab\n");
    search(&mut editor, "ab");
    editor.isearch_repeat(false);
    let mut cells = super::MatchCells::default();
    editor.isearch_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
    let cell = |row, column| CellRect {
        row,
        column,
        rows: 1,
        columns: 2,
    };
    assert_eq!(cells.current, [cell(1, 1)]);
    assert_eq!(cells.others, [cell(0, 0), cell(1, 4)]);
    editor.isearch_exit();
    editor.isearch_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
    assert!(cells.current.is_empty() && cells.others.is_empty());
}

#[test]
fn a_secret_buffer_searches_but_keeps_its_string_out_of_the_history() {
    let mut buffer = secret_buffer();
    buffer.apply_load(crate::load::decode_all(b"TOKEN=abc", Default::default()));
    let mut editor = Editor::with_buffer(buffer);
    search(&mut editor, "abc");
    assert!(editor.isearch().unwrap().around_secret());
    assert!(editor.around_secret());
    assert!(!format!("{:?}", editor.isearch().unwrap()).contains("abc"));
    editor.isearch_exit();
    editor.isearch_begin(false, false);
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "");
}

#[test]
fn a_prompt_has_no_isearch() {
    let mut editor = editor("abc");
    editor
        .open_prompt(
            "Go to line: ",
            crate::minibuffer::Kind::Number { history: None },
        )
        .unwrap();
    assert!(!editor.isearch_begin(false, false));
    assert!(editor.isearch().is_none());
}

#[test]
fn showing_another_buffer_ends_the_search_and_its_worker() {
    let text = format!("{}needle", "x".repeat(KEY_BUDGET * 2));
    let mut editor = editor(&text);
    editor.isearch_begin(false, false);
    editor.isearch_add("needle");
    let job = editor.take_isearch_job().unwrap();
    let other = editor.add_buffer(Buffer::from_text("other", "plain"));
    editor.show(other);
    assert!(editor.isearch().is_none());
    assert!(job.superseded());
    let mut reports = Vec::new();
    job.run(|report| reports.push(report));
    assert!(reports.is_empty());
    assert_eq!(cursor(&editor), 0);
}

#[test]
fn a_prompt_over_a_search_ends_it() {
    let mut editor = editor("abc abc");
    search(&mut editor, "b");
    let kind = crate::minibuffer::Kind::Number { history: None };
    editor.open_prompt("Go to line: ", kind).unwrap();
    assert!(editor.isearch().is_none());
    assert!(!editor.isearch_follow_text());
}

#[test]
fn a_secret_kill_in_the_string_makes_the_searched_buffer_secret() {
    let mut key = secret_buffer();
    key.apply_load(crate::load::decode_all(b"KEY=abc", Default::default()));
    let mut editor = Editor::with_buffer(key);
    let direction = crate::kill::Direction::Forward;
    editor.kill(std::iter::once(4..7), direction, true).unwrap();
    let plain = editor.add_buffer(Buffer::from_text("plain", "x abc"));
    editor.show(plain);
    assert!(!editor.current().1.is_secret());
    editor.isearch_begin(false, false);
    editor.isearch_yank_kill();
    assert!(editor.current().1.is_secret());
    assert!(editor.isearch().unwrap().around_secret());
    editor.isearch_exit();
    editor.isearch_begin(false, false);
    editor.isearch_history(true);
    assert_eq!(
        editor.isearch().unwrap().string(),
        "",
        "kept out of the history"
    );
}

#[test]
fn a_yank_copies_no_more_of_a_kill_than_the_string_holds() {
    let mut editor = editor("x");
    let long = crate::rope::Rope::from("y".repeat(MAX_QUERY_CHARS * 8).as_str());
    let pieces = vec![
        crate::kill::Piece::from(long),
        crate::kill::Piece::from(crate::rope::Rope::from("z")),
    ];
    editor.kills.ring.push(pieces, false);
    editor.isearch_begin(false, false);
    editor.isearch_add("ab");
    editor.isearch_yank_kill();
    assert_eq!(
        editor.isearch().unwrap().string().chars().count(),
        MAX_QUERY_CHARS
    );
}

#[test]
fn an_exit_around_a_secret_keeps_its_message_out_of_the_messages() {
    let mut key = secret_buffer();
    key.apply_load(crate::load::decode_all(b"KEY=abc", Default::default()));
    let mut editor = Editor::with_buffer(key);
    search(&mut editor, "abc");
    editor.isearch_exit();
    let messages = editor.buffer(editor.messages_buffer()).unwrap().text();
    let kept: String = messages.chunks().collect();
    assert!(!kept.contains("Mark saved"), "{kept}");
    assert_eq!(line(&editor), "Mark saved where search started");
}

#[test]
fn a_load_holds_the_count_until_it_ends() {
    let mut builder = crate::load::Builder::new(None, Default::default());
    builder.push(b"ab ab ");
    let mut buffer = Buffer::new("loading");
    buffer.apply_load(builder.snapshot());
    let mut editor = Editor::with_buffer(buffer);
    search(&mut editor, "ab");
    assert_eq!(line(&editor), "I-search: ab [1/2]");
    let id = editor.window().buffer();
    builder.push(b"ab ");
    editor
        .buffer_mut(id)
        .unwrap()
        .apply_load(builder.snapshot());
    assert!(editor.isearch_follow_text());
    assert_eq!(line(&editor), "I-search: ab [?/…]");
    assert!(editor.take_isearch_job().is_none(), "nothing counts a part");
    builder.push(b"ab");
    editor
        .buffer_mut(id)
        .unwrap()
        .apply_load(builder.snapshot());
    assert!(!editor.isearch_follow_text());
    builder.finish();
    editor
        .buffer_mut(id)
        .unwrap()
        .apply_load(builder.snapshot());
    assert!(editor.isearch_follow_text());
    drain(&mut editor);
    assert_eq!(line(&editor), "I-search: ab [1/4]");
}

#[test]
fn a_near_match_after_a_far_one_ends_the_wait_for_the_worker() {
    let text = format!("foo fa{}Foo", "x".repeat(KEY_BUDGET * 2));
    let mut editor = editor(&text);
    editor.isearch_begin(false, false);
    editor.isearch_add("F");
    assert!(editor.isearch().unwrap().searching());
    editor.isearch_toggle_case();
    assert!(!editor.isearch().unwrap().searching());
    assert_eq!(cursor(&editor), 1);
    drain(&mut editor);
    editor.isearch_repeat(false);
    assert_eq!(
        cursor(&editor),
        5,
        "the next f, as a repeat no longer waits"
    );
}

#[test]
fn delete_after_the_history_brings_the_string_back() {
    let mut editor = editor("one two");
    search(&mut editor, "two");
    editor.isearch_exit();
    editor.window_mut(0).0.selections = SelectionSet::new(0);
    editor.isearch_begin(false, false);
    editor.isearch_add("on");
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "two");
    editor.isearch_delete();
    assert_eq!(editor.isearch().unwrap().string(), "on");
    assert_eq!(cursor(&editor), 2);
}

#[test]
fn a_repeat_the_worker_goes_on_with_survives_the_end_of_a_load() {
    let mut builder = crate::load::Builder::new(None, Default::default());
    let far = format!("ab {}ab", "x".repeat(KEY_BUDGET * 2));
    builder.push(far.as_bytes());
    let mut buffer = Buffer::new("loading");
    buffer.apply_load(builder.snapshot());
    let mut editor = Editor::with_buffer(buffer);
    search(&mut editor, "ab");
    assert_eq!(cursor(&editor), 2);
    editor.isearch_repeat(false);
    assert!(editor.isearch().unwrap().searching());
    builder.push(b"!");
    builder.finish();
    let id = editor.window().buffer();
    editor
        .buffer_mut(id)
        .unwrap()
        .apply_load(builder.snapshot());
    assert!(editor.isearch_follow_text());
    drain(&mut editor);
    assert_eq!(
        cursor(&editor),
        far.len(),
        "the far match, not the one it repeated from"
    );
}

#[test]
fn an_overlapping_match_takes_the_index_of_the_counted_one_before_it() {
    let mut editor = editor("aaa");
    editor.window_mut(0).0.selections = SelectionSet::new(3);
    editor.isearch_begin(true, false);
    editor.isearch_add("aa");
    drain(&mut editor);
    assert_eq!(cursor(&editor), 1);
    assert_eq!(line(&editor), "I-search backward: aa [1/1]");
}
