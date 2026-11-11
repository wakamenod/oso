//! Tests of query-replace on the editor (ADR 0113): the answers, case, the scope, undo as one
//! step, `!` a slice at a time, the worker past a key's budget, and how it ends.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::super::test_buffers::secret_buffer;
use super::{ReplaceAnswer, ReplaceError};
use crate::buffer::{Buffer, ReadOnly};
use crate::editor::{Editor, Quit, Status};
use crate::search::KEY_BUDGET;
use crate::selection::{MarkActive, Selection, SelectionSet};
use crate::window::ViewSize;

use ReplaceAnswer::{All, Back, Exit, Replace, ReplaceAndExit, ReplaceAndStay, Skip, Undo};

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_view_size(ViewSize::new(10, 80));
    editor
}

/// Runs the worker's jobs in place, as the host's thread would, and takes in their reports.
fn drain(editor: &mut Editor) {
    while let Some(job) = editor.take_isearch_job() {
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        for report in reports {
            editor.isearch_reported(report);
        }
    }
}

/// Begins, as the command does, in an undo group of its own.
fn begin(editor: &mut Editor, from: &str, to: &str) {
    editor.begin_undo_group();
    editor.replace_begin(from, to, false).unwrap();
    editor.end_undo_group();
}

/// Answers, as a key does: a command of its own.
fn answer(editor: &mut Editor, answer: ReplaceAnswer) {
    editor.begin_undo_group();
    editor.replace_answer(answer);
    editor.end_undo_group();
}

/// Runs `!`'s slices, as the host's turns do.
fn finish_all(editor: &mut Editor) {
    while editor.replace_works() {
        editor.replace_continue();
    }
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn cursor(editor: &Editor) -> usize {
    editor.window().selections.primary().cursor
}

fn line(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

#[test]
fn each_answer_replaces_or_skips_and_the_end_says_how_many() {
    let mut editor = editor("foo bar foo foo");
    begin(&mut editor, "foo", "baz");
    drain(&mut editor);
    assert_eq!(cursor(&editor), 3, "at the end of the first match");
    assert_eq!(
        line(&editor),
        "Query replacing foo with baz: [1/3] (? for help)"
    );
    answer(&mut editor, Replace);
    assert_eq!(text(&editor), "baz bar foo foo");
    assert_eq!(cursor(&editor), 11);
    answer(&mut editor, Skip);
    answer(&mut editor, Replace);
    assert_eq!(text(&editor), "baz bar foo baz");
    assert!(editor.replace().is_none(), "no match left");
    assert_eq!(line(&editor), "Replaced 2 occurrences (skipped 1)");
}

#[test]
fn the_replacement_takes_the_case_of_the_match_while_the_search_folds() {
    let mut editor = editor("Foo FOO foo");
    begin(&mut editor, "foo", "bar");
    answer(&mut editor, All);
    finish_all(&mut editor);
    assert_eq!(text(&editor), "Bar BAR bar");
    // A capital in the string makes it case-sensitive, and the case is left as typed.
    let mut sensitive = self::editor("Foo FOO foo");
    begin(&mut sensitive, "Foo", "bar");
    answer(&mut sensitive, All);
    finish_all(&mut sensitive);
    assert_eq!(text(&sensitive), "bar FOO foo");
}

#[test]
fn a_session_undoes_as_one_step_and_u_undoes_one_replacement() {
    let mut editor = editor("a a a a");
    begin(&mut editor, "a", "b");
    answer(&mut editor, Replace);
    answer(&mut editor, Replace);
    assert_eq!(text(&editor), "b b a a");
    answer(&mut editor, Undo);
    assert_eq!(text(&editor), "b a a a");
    assert_eq!(cursor(&editor), 3, "back at the match undone");
    answer(&mut editor, Replace);
    answer(&mut editor, Exit);
    assert_eq!(text(&editor), "b b a a");
    editor.undo().unwrap();
    assert_eq!(text(&editor), "a a a a", "the whole session at once");
}

#[test]
fn dot_ends_after_one_and_comma_stays() {
    let mut editor = editor("x x x");
    begin(&mut editor, "x", "yy");
    answer(&mut editor, ReplaceAndStay);
    assert_eq!(text(&editor), "yy x x");
    assert_eq!(cursor(&editor), 2);
    assert!(editor.replace().is_some());
    // The next answer moves on, replacing nothing where it stayed.
    answer(&mut editor, Replace);
    assert_eq!(text(&editor), "yy x x");
    assert_eq!(cursor(&editor), 4);
    answer(&mut editor, ReplaceAndExit);
    assert_eq!(text(&editor), "yy yy x");
    assert!(editor.replace().is_none());
}

#[test]
fn caret_goes_back_to_the_match_before() {
    let mut editor = editor("x x x");
    begin(&mut editor, "x", "y");
    answer(&mut editor, Skip);
    assert_eq!(cursor(&editor), 3);
    answer(&mut editor, Back);
    assert_eq!(cursor(&editor), 1, "the skipped one, asked again");
    answer(&mut editor, Replace);
    answer(&mut editor, Back);
    assert_eq!(cursor(&editor), 1, "the replaced one, shown");
    answer(&mut editor, Skip);
    assert_eq!(cursor(&editor), 3, "and on from there");
}

#[test]
fn a_region_is_the_scope_and_several_regions_are_too() {
    let mut editor = editor("a a a a a");
    editor.current_mut().0.selections = SelectionSet::from_parts(
        vec![
            Selection {
                cursor: 3,
                mark: Some(0),
                goal: None,
            },
            Selection {
                cursor: 9,
                mark: Some(8),
                goal: None,
            },
        ],
        0,
    )
    .unwrap();
    editor
        .current_mut()
        .0
        .selections
        .set_mark_active(MarkActive::Active);
    begin(&mut editor, "a", "b");
    drain(&mut editor);
    assert_eq!(editor.replace().unwrap().count(), Some((1, 3)));
    answer(&mut editor, All);
    finish_all(&mut editor);
    assert_eq!(text(&editor), "b b a a b");
}

#[test]
fn with_no_region_it_runs_from_the_cursor_to_the_end() {
    let mut editor = editor("a a a");
    editor.current_mut().0.selections = SelectionSet::new(2);
    begin(&mut editor, "a", "b");
    answer(&mut editor, All);
    finish_all(&mut editor);
    assert_eq!(text(&editor), "a b b");
}

#[test]
fn an_empty_string_takes_the_last_pair_and_a_read_only_buffer_refuses() {
    let mut editor = editor("a a");
    assert_eq!(
        editor.replace_begin("", "", false),
        Err(ReplaceError::EmptyString)
    );
    begin(&mut editor, "a", "b");
    answer(&mut editor, Exit);
    assert_eq!(editor.last_replace(false), Some(("a", "b")));
    begin(&mut editor, "", "");
    assert_eq!(editor.replace().unwrap().to(), "b");

    let mut buffer = Buffer::visiting("/nowhere/notes.txt".into());
    let mut snapshot = crate::load::decode_all(b"a", crate::display_map::TabWidth::default());
    snapshot.done = false;
    buffer.apply_load(snapshot);
    let mut editor = Editor::with_buffer(buffer);
    assert_eq!(
        editor.replace_begin("a", "b", false),
        Err(ReplaceError::ReadOnly(ReadOnly::Loading))
    );
}

#[test]
fn past_a_keys_budget_the_worker_finds_and_answers_wait_for_it() {
    let far = format!("{}x x", ".".repeat(2 * KEY_BUDGET));
    let mut editor = editor(&far);
    begin(&mut editor, "x", "y");
    assert!(editor.replace().unwrap().searching());
    assert!(line(&editor).contains("[searching…]"), "{}", line(&editor));
    // An answer given meanwhile waits for the match.
    answer(&mut editor, Replace);
    drain(&mut editor);
    assert!(text(&editor).ends_with("y x"));
    assert_eq!(editor.replace().unwrap().count(), Some((2, 2)));
}

#[test]
fn bang_replaces_a_slice_at_a_time_and_undoes_as_one_step() {
    let many = "ab ".repeat(5000);
    let mut editor = editor(&many);
    begin(&mut editor, "ab", "c");
    answer(&mut editor, All);
    assert!(editor.replace_works(), "more than one slice");
    let mut slices = 1;
    while editor.replace_works() {
        editor.replace_continue();
        slices += 1;
    }
    assert!(slices >= 5, "{slices}");
    assert_eq!(text(&editor), "c ".repeat(5000));
    assert_eq!(line(&editor), "Replaced 5000 occurrences");
    editor.undo().unwrap();
    assert_eq!(text(&editor), many);
}

#[test]
fn c_g_ends_it_keeping_what_it_replaced() {
    let mut editor = editor("a a");
    begin(&mut editor, "a", "b");
    answer(&mut editor, Replace);
    assert_eq!(editor.quit(), Quit::Replace);
    assert!(editor.replace().is_none());
    assert_eq!(text(&editor), "b a");
}

#[test]
fn around_a_secret_the_pair_is_not_kept() {
    let mut buffer = secret_buffer();
    buffer.apply_load(crate::load::decode_all(
        b"a a",
        crate::display_map::TabWidth::default(),
    ));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(10, 80));
    begin(&mut editor, "a", "b");
    assert!(editor.replace().unwrap().around_secret());
    answer(&mut editor, Exit);
    assert_eq!(editor.last_replace(false), None);
}

#[test]
fn the_count_comes_even_if_an_answer_came_first() {
    let mut editor = editor("a a a");
    begin(&mut editor, "a", "b");
    answer(&mut editor, Replace);
    drain(&mut editor);
    assert_eq!(editor.replace().unwrap().count(), Some((2, 3)));
}

/// Begins replacing a regexp, as the command does.
fn begin_regexp(editor: &mut Editor, from: &str, to: &str) {
    editor.begin_undo_group();
    editor.replace_begin(from, to, true).unwrap();
    editor.end_undo_group();
}

fn all_of_regexp(text: &str, from: &str, to: &str) -> String {
    let mut editor = editor(text);
    begin_regexp(&mut editor, from, to);
    answer(&mut editor, All);
    // A first match past a key's budget comes from the worker, and `!` waits for it.
    drain(&mut editor);
    finish_all(&mut editor);
    self::text(&editor)
}

#[test]
fn a_regexps_groups_go_into_the_replacement() {
    assert_eq!(
        all_of_regexp("key=value a=b", r"(\w+)=(?P<v>\w+)", "${v}=$1"),
        "value=key b=a"
    );
    assert_eq!(all_of_regexp("cost 5", r"\d", "$$$0"), "cost $5");
}

#[test]
fn a_regexp_keeps_to_lines_and_finds_empty_matches() {
    assert_eq!(all_of_regexp("ab\nab", "^a", "x"), "xb\nxb");
    assert_eq!(all_of_regexp("a\nb", "$", "!"), "a!\nb!");
    assert_eq!(all_of_regexp("a\nb", r"a\nb", "x"), "a\nb");
}

#[test]
fn a_regexp_has_smart_case_and_the_replacement_the_matchs_case() {
    assert_eq!(all_of_regexp("Foo foo FOO", "fo+", "bar"), "Bar bar BAR");
    assert_eq!(all_of_regexp("Foo foo", "Fo+", "bar"), "bar foo");
    // Folding, so the case follows the match, whatever an inline flag says of a part.
    assert_eq!(all_of_regexp("Foo foo", r"\w(?-i)oo", "x"), "X x");
}

#[test]
fn a_regexp_that_does_not_compile_says_why() {
    let mut editor = editor("a");
    let refused = editor.replace_begin("(", "x", true).unwrap_err();
    assert!(
        matches!(&refused, ReplaceError::Regexp(why) if why.starts_with("Invalid regexp")),
        "{refused:?}"
    );
    assert!(editor.replace().is_none());
}

#[test]
fn a_regexp_past_a_keys_budget_is_found_on_the_worker() {
    let far = format!("{}\nend 42", "x".repeat(2 * KEY_BUDGET));
    let mut editor = editor(&far);
    begin_regexp(&mut editor, r"\d+", "n");
    assert!(editor.replace().unwrap().searching());
    drain(&mut editor);
    assert_eq!(editor.replace().unwrap().count(), Some((1, 1)));
    answer(&mut editor, Replace);
    assert!(text(&editor).ends_with("end n"));
}

#[test]
fn bang_passes_a_line_longer_than_a_slices_budget() {
    let long = "x".repeat(2 * KEY_BUDGET);
    let text = format!("{long}a1 b2\nc3");
    let replaced = all_of_regexp(&text, r"[abc]\d", "N");
    let end = &replaced[long.len().min(replaced.len())..];
    assert_eq!(end, "N N\nN");
    assert!(replaced.starts_with(&long));
}

#[test]
fn the_count_is_of_the_scope_as_it_began_even_after_a_replacement() {
    let text = format!("a{}a a", ".".repeat(2 * KEY_BUDGET));
    let mut editor = editor(&text);
    begin(&mut editor, "a", "b");
    // The answer's search passes a key's budget before the count has come.
    answer(&mut editor, Replace);
    assert!(editor.replace().unwrap().searching());
    drain(&mut editor);
    assert_eq!(editor.replace().unwrap().count(), Some((2, 3)));
}

#[test]
fn caret_again_goes_further_back_over_replaced_matches() {
    let mut editor = editor("x x x x");
    begin(&mut editor, "x", "y");
    answer(&mut editor, Replace);
    answer(&mut editor, Replace);
    answer(&mut editor, Back);
    assert_eq!(cursor(&editor), 3);
    answer(&mut editor, Back);
    assert_eq!(cursor(&editor), 1);
    answer(&mut editor, Back);
    assert_eq!(cursor(&editor), 1, "none before the first");
}

#[test]
fn undoing_the_session_puts_the_cursor_back_where_it_began() {
    let mut editor = editor("foo bar foo");
    begin(&mut editor, "foo", "x");
    answer(&mut editor, Replace);
    answer(&mut editor, Replace);
    assert!(editor.replace().is_none());
    editor.undo().unwrap();
    assert_eq!(text(&editor), "foo bar foo");
    assert_eq!(cursor(&editor), 0);
    // Also after one replacement alone.
    let mut editor = self::editor("foo foo");
    begin(&mut editor, "foo", "x");
    answer(&mut editor, Replace);
    answer(&mut editor, Exit);
    editor.undo().unwrap();
    assert_eq!(cursor(&editor), 0);
}

#[test]
fn no_debug_says_the_strings() {
    let mut editor = editor("secret secret");
    begin(&mut editor, "secret", "hidden");
    let printed = format!("{:?} {:?}", editor.replace(), editor.replacing);
    assert!(
        !printed.contains("secret") && !printed.contains("hidden"),
        "{printed}"
    );
}

#[test]
fn answers_waiting_for_the_worker_are_few() {
    let far = format!("{}x", ".".repeat(2 * KEY_BUDGET));
    let mut editor = editor(&far);
    begin(&mut editor, "x", "y");
    for _ in 0..40 {
        answer(&mut editor, Skip);
    }
    assert_eq!(editor.replace().unwrap().pending.len(), super::MOST_PENDING);
}

#[test]
fn an_empty_replacement_goes_on_at_its_end() {
    let mut editor = editor("aab");
    begin(&mut editor, "a", "");
    answer(&mut editor, Replace);
    answer(&mut editor, Replace);
    assert_eq!(text(&editor), "b");
}

#[test]
fn a_session_the_undo_limit_cut_still_undoes_what_is_left_at_once() {
    let many = "ab ".repeat(5000);
    let mut editor = editor(&many);
    let id = editor.window().buffer();
    // Room for a slice or two of `!`'s, not all five.
    editor.buffer_mut(id).unwrap().set_undo_limit(400_000);
    begin(&mut editor, "ab", "c");
    answer(&mut editor, All);
    finish_all(&mut editor);
    assert_eq!(text(&editor), "c ".repeat(5000));
    editor.undo().unwrap();
    assert_ne!(text(&editor), "c ".repeat(5000), "it undid something");
    assert!(editor.undo().is_err(), "all that was left, in one step");
}

#[test]
fn an_answer_at_an_empty_match_moves_on_past_it() {
    let mut editor = editor("a\nb");
    begin_regexp(&mut editor, "$", "!");
    answer(&mut editor, Replace);
    answer(&mut editor, Replace);
    assert_eq!(text(&editor), "a!\nb!");
    assert!(editor.replace().is_none());
}
