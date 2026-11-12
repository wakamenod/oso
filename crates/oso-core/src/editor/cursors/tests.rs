//! Tests of the keys that add cursors (ADR 0091): what they look for and where, counts,
//! skips and removal, the bound, every occurrence, each line, `C-g`, and the worker.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::{CursorError, Toward};
use crate::buffer::{Buffer, Replacement};
use crate::editor::test_buffers::secret_buffer;
use crate::editor::{Editor, Quit, Status};
use crate::minibuffer::Kind;
use crate::search::KEY_BUDGET;
use crate::selection::{Goal, MAX_CURSORS, MarkActive, Selection, SelectionSet};

pub(super) fn editor(text: &str) -> Editor {
    Editor::with_buffer(Buffer::from_text("notes.txt", text))
}

/// The cursors' regions, as `(mark, cursor)`, in order, and which is primary.
pub(super) fn regions(editor: &Editor) -> (Vec<(usize, usize)>, usize) {
    let set = &editor.window().selections;
    let regions = set
        .iter()
        .map(|selection| (selection.mark.unwrap_or(selection.cursor), selection.cursor))
        .collect();
    (regions, set.primary_index())
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

/// The newest line of `*Messages*`.
fn last_message(editor: &Editor) -> String {
    let messages = editor.buffer(editor.messages_buffer()).unwrap().text();
    let kept: String = messages.chunks().collect();
    kept.lines().last().unwrap_or_default().to_owned()
}

pub(super) fn cursors(editor: &Editor) -> Vec<usize> {
    editor
        .window()
        .selections
        .iter()
        .map(|s| s.cursor)
        .collect()
}

/// One cursor at `cursor`, with its region from `mark` shown.
pub(super) fn region(editor: &mut Editor, mark: usize, cursor: usize) {
    let mut set = SelectionSet::from_parts(
        vec![Selection {
            cursor,
            mark: Some(mark),
            goal: None,
        }],
        0,
    )
    .unwrap();
    set.set_mark_active(MarkActive::Active);
    editor.windows[0].selections = set;
}

pub(super) fn at(editor: &mut Editor, cursors: &[usize]) {
    let parts = cursors.iter().copied().map(Selection::at).collect();
    editor.windows[0].selections = SelectionSet::from_parts(parts, 0).unwrap();
}

/// Runs the worker's jobs in place, as the host's thread would, and takes in what they report.
pub(super) fn drain(editor: &mut Editor) {
    while let Some(job) = editor.take_isearch_job() {
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        for report in reports {
            editor.isearch_reported(report);
        }
    }
}

#[test]
fn the_first_key_selects_the_word_and_adds_its_next_occurrence() {
    let mut editor = editor("foo bar foo baz foo");
    at(&mut editor, &[1]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 3), (8, 11)], 1));
    assert!(editor.window().selections.mark_active().is_active());
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 3), (8, 11), (16, 19)], 2));
    assert_eq!(
        editor.mark_like_this(Toward::Next, 1),
        Err(CursorError::NoMore),
        "no wrap past the end"
    );
    assert_eq!(regions(&editor).0.len(), 3);
}

#[test]
fn a_word_finds_only_whole_words_and_a_region_any_match() {
    let mut editor = editor("id idle id");
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor).0, [(0, 2), (8, 10)]);

    let mut editor = self::editor("id idle id");
    region(&mut editor, 0, 2);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor).0, [(0, 2), (3, 5)]);
}

#[test]
fn a_region_matches_its_case_exactly_and_keeps_its_direction() {
    let mut editor = editor("Foo foo Foo");
    region(&mut editor, 3, 0);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor), (vec![(3, 0), (11, 8)], 1));
}

#[test]
fn previous_goes_back_from_the_first_cursor() {
    let mut editor = editor("ab x ab x ab");
    at(&mut editor, &[10]);
    editor.mark_like_this(Toward::Previous, 1).unwrap();
    assert_eq!(regions(&editor), (vec![(5, 7), (10, 12)], 0));
    editor.mark_like_this(Toward::Previous, 1).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 2), (5, 7), (10, 12)], 0));
    assert_eq!(
        editor.mark_like_this(Toward::Previous, 1),
        Err(CursorError::NoMore)
    );
}

#[test]
fn a_count_adds_that_many_and_says_when_it_found_fewer() {
    let mut editor = editor("a a a a");
    region(&mut editor, 0, 1);
    editor.mark_like_this(Toward::Next, 2).unwrap();
    assert_eq!(cursors(&editor), [1, 3, 5]);
    assert_eq!(
        editor.mark_like_this(Toward::Next, 3),
        Err(CursorError::NoMore)
    );
    assert_eq!(cursors(&editor), [1, 3, 5, 7], "what it found, it added");
}

#[test]
fn a_negative_count_removes_from_that_end_and_keeps_one() {
    let mut editor = editor("a a a a");
    region(&mut editor, 0, 1);
    editor.mark_like_this(Toward::Next, 3).unwrap();
    editor.mark_like_this(Toward::Next, -1).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 1), (2, 3), (4, 5)], 2));
    editor.mark_like_this(Toward::Previous, -1).unwrap();
    assert_eq!(regions(&editor), (vec![(2, 3), (4, 5)], 1));
    editor.mark_like_this(Toward::Next, -5).unwrap();
    assert_eq!(regions(&editor).0, [(2, 3)]);
    assert_eq!(
        editor.mark_like_this(Toward::Next, -1),
        Err(CursorError::OnlyOne)
    );
}

#[test]
fn a_skip_moves_the_last_cursor_on_and_zero_does_as_well() {
    let mut editor = editor("a a a a");
    region(&mut editor, 0, 1);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    editor.skip_like_this(Toward::Next).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 1), (4, 5)], 1));
    editor.mark_like_this(Toward::Next, 0).unwrap();
    assert_eq!(regions(&editor), (vec![(0, 1), (6, 7)], 1));
    assert_eq!(
        editor.skip_like_this(Toward::Next),
        Err(CursorError::NoMore)
    );
    assert_eq!(regions(&editor).0, [(0, 1), (6, 7)], "nothing moves");

    let mut editor = self::editor("a a");
    region(&mut editor, 0, 1);
    editor.skip_like_this(Toward::Next).unwrap();
    assert_eq!(regions(&editor), (vec![(2, 3)], 0), "one cursor moves");
}

#[test]
fn no_word_and_a_long_region_are_refused() {
    let mut editor = editor("  a");
    at(&mut editor, &[0]);
    assert_eq!(
        editor.mark_like_this(Toward::Next, 1),
        Err(CursorError::NoWord)
    );
    let long = "x".repeat(2000);
    let mut editor = self::editor(&long);
    region(&mut editor, 0, 1025);
    assert_eq!(
        editor.mark_like_this(Toward::Next, 1),
        Err(CursorError::TooLong)
    );
}

#[test]
fn no_key_passes_the_bound() {
    let text = "a ".repeat(MAX_CURSORS + 10);
    let mut editor = editor(&text);
    region(&mut editor, 0, 1);
    editor.mark_like_this(Toward::Next, 998).unwrap();
    // A count shares one key's budget; the worker finds the rest.
    assert!(cursors(&editor).len() < 999);
    drain(&mut editor);
    assert_eq!(cursors(&editor).len(), 999);
    assert_eq!(
        editor.mark_like_this(Toward::Next, 2),
        Err(CursorError::TooMany)
    );
    assert_eq!(cursors(&editor).len(), 999, "adds none");
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(cursors(&editor).len(), MAX_CURSORS);
}

#[test]
fn every_occurrence_gets_a_cursor_and_the_primary_stays() {
    let mut editor = editor("x foo foo xfoo foo");
    at(&mut editor, &[6]);
    editor.mark_all_like_this().unwrap();
    assert_eq!(
        regions(&editor),
        (vec![(2, 5), (6, 9), (15, 18)], 1),
        "whole words only"
    );
}

#[test]
fn more_occurrences_than_the_bound_add_none_and_say_how_many() {
    let text = "a ".repeat(MAX_CURSORS + 1);
    let mut editor = editor(&text);
    region(&mut editor, 0, 1);
    editor.mark_all_like_this().unwrap();
    drain(&mut editor);
    assert_eq!(cursors(&editor), [1]);
    assert_eq!(
        last_message(&editor),
        format!("{} matches; at most 1000 cursors", MAX_CURSORS + 1)
    );
    assert_eq!(
        CursorError::TooManyMatches(1234).to_string(),
        "1234 matches; at most 1000 cursors"
    );
}

#[test]
fn edit_lines_puts_one_on_each_line_at_the_goal_column() {
    let mut editor = editor("abcd\nab\nabcd\n");
    let mut set = SelectionSet::from_parts(
        vec![Selection {
            cursor: 11,
            mark: Some(2),
            goal: None,
        }],
        0,
    )
    .unwrap();
    set.set_mark_active(MarkActive::Active);
    editor.windows[0].selections = set;
    editor.edit_lines().unwrap();
    assert_eq!(cursors(&editor), [3, 7, 11], "the short line at its end");
    let set = &editor.window().selections;
    assert_eq!(set.primary_index(), 2);
    assert!(!set.mark_active().is_active());
    assert!(set.iter().all(|s| s.goal == Some(Goal::Column(3))));

    let mut editor = self::editor("ab\nab");
    at(&mut editor, &[1]);
    assert_eq!(editor.edit_lines(), Err(CursorError::NoMark));
}

#[test]
fn edit_lines_refuses_more_lines_than_the_bound() {
    let text = "a\n".repeat(MAX_CURSORS + 1);
    let mut editor = editor(&text);
    region(&mut editor, 0, text.len());
    assert_eq!(editor.edit_lines(), Err(CursorError::TooMany));
    assert_eq!(cursors(&editor), [text.len()]);
}

#[test]
fn quit_hides_the_regions_first_then_leaves_the_primary_alone() {
    let mut editor = editor("foo foo foo");
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 2).unwrap();
    assert_eq!(editor.quit(), Quit::Region);
    assert_eq!(cursors(&editor), [3, 7, 11]);
    assert_eq!(editor.quit(), Quit::Cursors);
    assert_eq!(cursors(&editor), [11]);
    assert_eq!(editor.quit(), Quit::Nothing);
}

#[test]
fn quit_closes_a_prompt_before_it_touches_the_cursors() {
    let mut editor = editor("foo foo");
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    editor.deactivate_mark();
    editor
        .open_prompt("Go to line: ", Kind::Number { history: None })
        .unwrap();
    assert_eq!(editor.quit(), Quit::Prompt);
    assert_eq!(cursors(&editor), [3, 7]);
}

/// Text with `word` past what a key searches on the UI thread.
fn far(word: &str) -> String {
    format!("{word} {}{word}", "-".repeat(KEY_BUDGET * 2))
}

#[test]
fn a_far_occurrence_comes_from_the_worker() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(cursors(&editor), [6], "the word is selected at once");
    drain(&mut editor);
    let end = text.len();
    assert_eq!(regions(&editor), (vec![(0, 6), (end - 6, end)], 1));
}

#[test]
fn every_occurrence_of_a_large_buffer_comes_from_the_worker() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_all_like_this().unwrap();
    drain(&mut editor);
    assert_eq!(cursors(&editor), [6, text.len()]);
}

#[test]
fn a_worker_answer_is_dropped_once_the_cursors_moved() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    editor.deactivate_mark();
    drain(&mut editor);
    assert_eq!(cursors(&editor), [6]);
}

#[test]
fn a_worker_answer_is_dropped_once_the_text_changed() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    let id = editor.window().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    let selections = editor.windows[0].selections.clone();
    editor
        .apply_edits(
            id,
            &[Replacement {
                range: end..end,
                text: "!",
            }],
        )
        .unwrap();
    editor.windows[0].selections = selections;
    drain(&mut editor);
    assert_eq!(cursors(&editor), [6]);
}

#[test]
fn dropping_a_search_supersedes_its_job() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    let job = editor.take_isearch_job().unwrap();
    editor.drop_cursor_search();
    assert_eq!(echo(&editor), "", "it says nothing of the search any more");
    let mut reports = Vec::new();
    job.run(|report| reports.push(report));
    assert!(reports.is_empty(), "the job was superseded");
}

#[test]
fn a_run_of_keys_keeps_its_query() {
    let mut editor = editor("ab ab abc ab");
    region(&mut editor, 0, 2);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    // The primary's region now covers the second `ab`; the query stays the region's.
    editor.mark_like_this(Toward::Next, 2).unwrap();
    assert_eq!(regions(&editor).0, [(0, 2), (3, 5), (6, 8), (10, 12)]);
}

#[test]
fn what_the_worker_says_around_a_secret_stays_out_of_the_messages() {
    let mut key = secret_buffer();
    let text = format!("KEY {}", "-".repeat(KEY_BUDGET * 2));
    key.apply_load(crate::load::decode_all(text.as_bytes(), Default::default()));
    let mut editor = Editor::with_buffer(key);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    drain(&mut editor);
    let messages = editor.buffer(editor.messages_buffer()).unwrap().text();
    let kept: String = messages.chunks().collect();
    assert!(!kept.contains("No more matches"), "{kept}");
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    assert_eq!(status.echo, "No more matches");
}

#[test]
fn every_occurrence_waits_for_a_load_to_end() {
    let mut builder = crate::load::Builder::new(None, Default::default());
    builder.push(b"ab ab ");
    let mut buffer = Buffer::new("loading");
    buffer.apply_load(builder.snapshot());
    let mut editor = Editor::with_buffer(buffer);
    region(&mut editor, 0, 2);
    assert_eq!(editor.mark_all_like_this(), Err(CursorError::Loading));
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(cursors(&editor), [2, 5], "what has loaded is searched");
}

#[test]
fn a_match_inside_a_cluster_is_no_occurrence() {
    // `e` before a combining mark, an emoji inside a ZWJ sequence, and LF inside CRLF.
    let mut editor = editor("e e\u{301} e");
    region(&mut editor, 0, 1);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor).0, [(0, 1), (6, 7)]);

    let text = "👩 👩\u{200d}💻 👩";
    let mut editor = self::editor(text);
    region(&mut editor, 0, 4);
    editor.mark_all_like_this().unwrap();
    let last = text.len() - 4;
    assert_eq!(regions(&editor).0, [(0, 4), (last, text.len())]);

    let mut editor = self::editor("a\nfoo\r\nfoo\nfoo");
    region(&mut editor, 1, 5);
    editor.mark_all_like_this().unwrap();
    assert_eq!(regions(&editor).0, [(1, 5), (10, 14)], "not inside CRLF");
}

#[test]
fn a_query_goes_with_the_cursors_that_shared_it() {
    let mut editor = editor("foo baz x baz");
    region(&mut editor, 0, 3);
    assert_eq!(
        editor.mark_like_this(Toward::Next, 1),
        Err(CursorError::NoMore)
    );
    editor.quit();
    region(&mut editor, 4, 7);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(regions(&editor).0, [(4, 7), (10, 13)]);
}

#[test]
fn rejected_words_back_from_the_end_cost_a_keys_budget_at_most() {
    // Every `i` is inside `fix` but the last, so each match before it is checked and
    // rejected; the key hands the rest to the worker rather than read them all.
    let text = format!("{}i", "fix ".repeat(4000));
    let mut editor = editor(&text);
    at(&mut editor, &[text.len()]);
    editor.mark_like_this(Toward::Previous, 1).unwrap();
    assert!(editor.take_isearch_job().is_some(), "the worker goes on");
    let mut editor = self::editor(&text);
    at(&mut editor, &[text.len()]);
    editor.mark_like_this(Toward::Previous, 1).unwrap();
    drain(&mut editor);
    assert_eq!(cursors(&editor), [text.len()]);
    assert_eq!(last_message(&editor), "No more matches");
}

#[test]
fn a_count_shares_one_budget() {
    // Matches far apart: a count of 3 cannot read past one key's budget on this thread.
    let gap = "-".repeat(KEY_BUDGET / 2);
    let text = format!("ab{gap}ab{gap}ab{gap}ab");
    let mut editor = editor(&text);
    region(&mut editor, 0, 2);
    editor.mark_like_this(Toward::Next, 3).unwrap();
    assert!(cursors(&editor).len() < 4);
    drain(&mut editor);
    assert_eq!(cursors(&editor).len(), 4);
}

#[test]
fn a_search_begun_after_the_ask_drops_the_answer() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    let job = editor.take_isearch_job().unwrap();
    editor.isearch_begin(false, false);
    let mut reports = Vec::new();
    job.run(|report| reports.push(report));
    for report in reports {
        editor.isearch_reported(report);
    }
    assert_eq!(cursors(&editor), [6]);
}

#[test]
fn an_answer_stands_through_a_load_where_the_load_cannot_reach() {
    let gap = "-".repeat(KEY_BUDGET * 2);
    let tail = "-".repeat(2000);
    let mut builder = crate::load::Builder::new(None, Default::default());
    builder.push(format!("needle {gap}needle {tail}needle").as_bytes());
    let mut buffer = Buffer::new("loading");
    buffer.apply_load(builder.snapshot());
    let mut editor = Editor::with_buffer(buffer);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 2).unwrap();
    // More arrives before the worker answers: the last match is too near the end.
    builder.push(b" more");
    let id = editor.window().buffer();
    editor
        .buffer_mut(id)
        .unwrap()
        .apply_load(builder.snapshot());
    drain(&mut editor);
    assert_eq!(echo(&editor), "Search dropped: the buffer changed");
    assert_eq!(cursors(&editor), [6]);

    let mut builder = crate::load::Builder::new(None, Default::default());
    builder.push(format!("needle {gap}needle {tail}").as_bytes());
    let mut buffer = Buffer::new("loading");
    buffer.apply_load(builder.snapshot());
    let mut editor = Editor::with_buffer(buffer);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    builder.push(b" more");
    let id = editor.window().buffer();
    editor
        .buffer_mut(id)
        .unwrap()
        .apply_load(builder.snapshot());
    drain(&mut editor);
    assert_eq!(
        cursors(&editor).len(),
        2,
        "far enough from where the load appends"
    );
}

#[test]
fn quit_ends_a_search_before_it_takes_the_cursors_away() {
    let mut editor = editor("ab ab ab");
    region(&mut editor, 0, 2);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    editor.deactivate_mark();
    editor.isearch_begin(false, false);
    editor.isearch_add("ab");
    drain(&mut editor);
    assert_eq!(editor.quit(), Quit::Search);
    assert_eq!(cursors(&editor).len(), 2);
    assert_eq!(editor.quit(), Quit::Cursors);
}

#[test]
fn a_successful_answer_clears_searching() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(echo(&editor), "Searching…");
    drain(&mut editor);
    assert_eq!(echo(&editor), "");
}

#[test]
fn quit_alone_leaves_a_persons_search_for_the_person() {
    // The agent channel's cancel calls `quit`; only a person's `C-g` drops the search.
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    assert_eq!(editor.quit(), Quit::Region);
    assert!(editor.take_isearch_job().is_some(), "the job still runs");
}

#[test]
fn an_answer_for_cursors_that_moved_goes_with_nothing_said() {
    let text = far("needle");
    let mut editor = editor(&text);
    at(&mut editor, &[0]);
    editor.mark_like_this(Toward::Next, 1).unwrap();
    editor.deactivate_mark();
    drain(&mut editor);
    assert_eq!(cursors(&editor), [6]);
    assert_eq!(echo(&editor), "");
    assert_ne!(last_message(&editor), "Search dropped: the buffer changed");
}
