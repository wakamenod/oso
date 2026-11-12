//! Tests of regexp isearch on the editor (ADR 0116): its line, incomplete and wrong regexps,
//! `C-g` past them, backward search, empty matches, its own history, `M-r`, smart case, a
//! regexp compiled on the worker, long lines, and its highlights.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use std::sync::atomic::Ordering;

use super::test_buffers::secret_buffer;
use super::{Editor, Quit, Status};
use crate::buffer::Buffer;
use crate::frame::CellRect;
use crate::search::{KEY_BUDGET, Report, SearchReport};
use crate::selection::SelectionSet;
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

/// Starts a regexp search forward for `pattern`, and lets the worker count.
fn search(editor: &mut Editor, pattern: &str) {
    editor.isearch_begin(false, true);
    editor.isearch_add(pattern);
    drain(editor);
}

#[test]
fn typing_a_regexp_goes_to_the_end_of_its_match_and_counts() {
    let mut editor = editor("foo1 foo22 fo");
    editor.isearch_begin(false, true);
    assert_eq!(line(&editor), "Regexp I-search: ");
    assert!(editor.isearch().unwrap().regexp());
    editor.isearch_add(r"fo+\d+");
    drain(&mut editor);
    assert_eq!(cursor(&editor), 4);
    assert_eq!(line(&editor), r"Regexp I-search: fo+\d+ [1/2]");
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 10);
    assert_eq!(line(&editor), r"Regexp I-search: fo+\d+ [2/2]");
    editor.isearch_repeat(false);
    assert!(line(&editor).starts_with("Failing regexp I-search: "));
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 4);
    assert_eq!(line(&editor), r"Wrapped regexp I-search: fo+\d+ [1/2]");
}

#[test]
fn an_incomplete_regexp_says_so_and_the_cursor_stays() {
    let mut editor = editor("abx [x] ab");
    search(&mut editor, "ab");
    assert_eq!(cursor(&editor), 2);
    editor.isearch_add("[");
    assert_eq!(line(&editor), "Regexp I-search: ab[ [incomplete input]");
    let state = editor.isearch().unwrap();
    assert!(!state.failing() && !state.searching() && state.count().is_none());
    assert_eq!(cursor(&editor), 2);
    let mut cells = super::MatchCells::default();
    editor.isearch_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
    assert!(cells.current.is_empty() && cells.others.is_empty());
    // Nothing to repeat while it does not compile.
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 2);
    editor.isearch_add("x]");
    drain(&mut editor);
    assert_eq!(cursor(&editor), 3);
    assert_eq!(line(&editor), "Regexp I-search: ab[x] [1/1]");
}

#[test]
fn a_wrong_regexp_says_why() {
    let mut editor = editor("a) b");
    search(&mut editor, "a)");
    assert_eq!(
        line(&editor),
        "Regexp I-search: a) [Invalid regexp: unopened group]"
    );
    assert_eq!(cursor(&editor), 0);
}

#[test]
fn c_g_goes_back_past_an_incomplete_regexp_then_quits() {
    let mut editor = editor("abc");
    search(&mut editor, "ab");
    editor.isearch_add("(c");
    assert_eq!(editor.quit(), Quit::SearchStep);
    let state = editor.isearch().unwrap();
    assert_eq!(state.string(), "ab");
    assert_eq!(line(&editor), "Regexp I-search: ab [1/1]");
    assert_eq!(editor.quit(), Quit::Search);
    assert_eq!(cursor(&editor), 0);
}

#[test]
fn backward_takes_the_match_that_starts_last_and_typing_keeps_it() {
    let mut editor = editor("xaaa");
    editor.window_mut(0).0.selections = SelectionSet::new(4);
    editor.isearch_begin(true, true);
    editor.isearch_add("aa");
    drain(&mut editor);
    // Emacs tries each start back from the cursor: `aa` at 2, inside the one at 1.
    assert_eq!(cursor(&editor), 2);
    assert_eq!(line(&editor), "Regexp I-search backward: aa [1/1]");
    // A repeat's match ends by this one's start, as `re-search-backward`'s ends by the point,
    // so the one at 1 is not taken.
    editor.isearch_repeat(true);
    assert!(editor.isearch().unwrap().failing());
    // A longer regexp that still matches at the match's start stays there.
    let mut editor = self::editor("foo x foo yz");
    editor.window_mut(0).0.selections = SelectionSet::new(11);
    editor.isearch_begin(true, true);
    editor.isearch_add("f");
    assert_eq!(cursor(&editor), 6);
    editor.isearch_add("o+");
    assert_eq!(cursor(&editor), 6);
    // One that would end past where the search began does not.
    editor.isearch_add(" y.");
    assert!(editor.isearch().unwrap().failing());
}

#[test]
fn empty_matches_go_as_the_count_takes_them() {
    let mut editor = editor("xx y");
    search(&mut editor, "x*");
    assert_eq!(cursor(&editor), 2);
    assert_eq!(line(&editor), "Regexp I-search: x* [1/3]");
    // The empty match where the first ended is passed, as `replace_all` passes it.
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 3);
    assert_eq!(line(&editor), "Regexp I-search: x* [2/3]");
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 4);
    assert_eq!(line(&editor), "Regexp I-search: x* [3/3]");
    editor.isearch_repeat(false);
    assert!(editor.isearch().unwrap().failing());
    // Backward from an empty match goes a character back first.
    editor.isearch_repeat(true);
    assert_eq!(cursor(&editor), 3);
    editor.isearch_repeat(true);
    assert_eq!(cursor(&editor), 2);
}

#[test]
fn the_regexp_history_is_kept_apart() {
    let mut editor = editor("a.c abc b");
    search(&mut editor, "a.c");
    editor.isearch_exit();
    editor.isearch_begin(false, false);
    editor.isearch_add("b");
    editor.isearch_exit();
    editor.isearch_begin(false, false);
    editor.isearch_repeat(false);
    assert_eq!(editor.isearch().unwrap().string(), "b");
    editor.isearch_exit();
    editor.isearch_begin(false, true);
    editor.isearch_repeat(false);
    assert_eq!(editor.isearch().unwrap().string(), "a.c");
    editor.isearch_exit();
    editor.isearch_begin(false, true);
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "a.c");
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "a.c", "only regexps");
}

#[test]
fn m_r_turns_the_string_around_and_del_turns_it_back() {
    let mut editor = editor("abc a.c");
    editor.isearch_begin(false, false);
    editor.isearch_add("a.");
    drain(&mut editor);
    assert_eq!(cursor(&editor), 6);
    editor.isearch_toggle_regexp();
    drain(&mut editor);
    // `a.` as a regexp still matches where the cursor is.
    assert_eq!(cursor(&editor), 6);
    assert_eq!(line(&editor), "Regexp I-search: a. [2/2]");
    editor.isearch_exit();
    // The history follows the search's kind when it ended.
    editor.isearch_begin(false, true);
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "a.");
    editor.isearch_toggle_regexp();
    assert!(!editor.isearch().unwrap().regexp());
    assert!(editor.isearch_delete());
    assert!(editor.isearch().unwrap().regexp());
}

#[test]
fn smart_case_looks_past_escapes_and_m_c_turns_it() {
    let mut editor = editor("FOO foo");
    search(&mut editor, r"\Wfoo");
    assert!(!editor.isearch().unwrap().case_sensitive());
    assert_eq!(cursor(&editor), 7);
    let mut editor = self::editor("foo FOO");
    search(&mut editor, "FOO");
    assert!(editor.isearch().unwrap().case_sensitive());
    assert_eq!(cursor(&editor), 7);
    editor.isearch_toggle_case();
    drain(&mut editor);
    assert_eq!(line(&editor), "Case-insensitive regexp I-search: FOO [2/2]");
    // An inline flag wins.
    let mut editor = self::editor("foo FOO");
    search(&mut editor, "(?-i)FOO|x");
    assert_eq!(cursor(&editor), 7);
}

#[test]
fn a_regexp_too_large_for_a_key_compiles_on_the_worker() {
    let word = "w".repeat(30);
    let mut editor = editor(&format!("x {word}"));
    editor.isearch_begin(false, true);
    editor.isearch_add(r"\w{30}");
    assert!(editor.isearch().unwrap().searching());
    assert_eq!(line(&editor), r"Regexp I-search: \w{30} [searching…]");
    assert_eq!(cursor(&editor), 0);
    drain(&mut editor);
    assert_eq!(cursor(&editor), 32);
    assert_eq!(line(&editor), r"Regexp I-search: \w{30} [1/1]");
    // Back to it, the worker compiles it again, for the highlights.
    editor.isearch_repeat(false);
    assert!(editor.isearch().unwrap().failing());
    assert!(editor.isearch_delete());
    assert!(editor.take_isearch_job().is_some());
    // One too large for the worker too says why.
    editor.isearch_add(r"\w{30}\w{30}\w{30}");
    drain(&mut editor);
    let said = line(&editor);
    assert!(said.contains("[Invalid regexp: "), "{said}");
}

#[test]
fn a_line_longer_than_a_key_s_budget_is_the_worker_s() {
    let text = format!("short\n{}needle", "x".repeat(KEY_BUDGET + 1));
    let mut editor = editor(&text);
    editor.isearch_begin(false, true);
    editor.isearch_add("ne+dle");
    assert!(editor.isearch().unwrap().searching());
    assert_eq!(cursor(&editor), 0);
    drain(&mut editor);
    assert_eq!(cursor(&editor), text.len());
    assert_eq!(line(&editor), "Regexp I-search: ne+dle [1/1]");
}

#[test]
fn a_line_too_long_fails_the_search_there() {
    let mut editor = editor("foo\na longer line\nbar o");
    search(&mut editor, "o");
    editor.isearch_longest_line(5);
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), 3);
    editor.isearch_repeat(false);
    let state = editor.isearch().unwrap();
    assert!(state.failing() && state.count().is_none());
    assert_eq!(
        line(&editor),
        "Failing regexp I-search: o [Regexp search stops at line 2: it is longer than 64 MiB]"
    );
    // The worker's count meeting one shows no count, and the search goes on.
    let mut editor = self::editor("foo");
    search(&mut editor, "o");
    let live = editor.search_live.load(Ordering::Relaxed);
    let report = SearchReport(Report::LineTooLong {
        generation: live,
        at: 0,
    });
    assert!(editor.isearch_reported(report));
    let state = editor.isearch().unwrap();
    assert!(!state.failing() && state.count().is_none());
    assert_eq!(line(&editor), "Regexp I-search: o");
}

#[test]
fn the_matches_of_a_regexp_in_view_are_cells() {
    let mut editor = editor("ab\nxac ad\n");
    search(&mut editor, "a.");
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
}

#[test]
fn a_yank_into_a_regexp_is_quoted() {
    let mut editor = editor("x.y xzy");
    editor.isearch_begin(false, true);
    editor.isearch_yank_word();
    editor.isearch_yank_word();
    drain(&mut editor);
    assert_eq!(editor.isearch().unwrap().string(), r"x\.");
    assert_eq!(line(&editor), r"Regexp I-search: x\. [1/1]");
}

#[test]
fn a_secret_buffer_keeps_a_regexp_out_of_the_history_and_debug() {
    let mut buffer = secret_buffer();
    buffer.apply_load(crate::load::decode_all(b"TOKEN=abc", Default::default()));
    let mut editor = Editor::with_buffer(buffer);
    search(&mut editor, "a.c");
    assert!(editor.isearch().unwrap().around_secret());
    assert!(!format!("{:?}", editor.isearch().unwrap()).contains("a.c"));
    editor.isearch_exit();
    editor.isearch_begin(false, true);
    editor.isearch_history(true);
    assert_eq!(editor.isearch().unwrap().string(), "");
}
