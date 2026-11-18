//! Tests of `consult-line`'s search of a buffer's lines (ADR 0154): the order from the cursor's
//! line, every line with no input, narrowing, the cap, blocks and long lines, the preview, what
//! `RET` and `C-g` leave, and a buffer that may hold secrets.
//!
//! Must not depend on: threads or the file system; a request runs here, as the host would.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use super::*;
use crate::buffer::Buffer;
use crate::editor::edit_tests::edit;
use crate::minibuffer::Outcome;
use crate::project::MAX_ROW_BYTES;
use crate::window::ViewSize;

/// An editor showing `text`, its cursor at `cursor`, with a line search open.
fn searching(text: &str, cursor: usize) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
    editor.set_view_size(ViewSize::new(30, 100));
    editor.current_mut().0.selections.move_primary(cursor);
    editor.open_line_search("consult-line").unwrap();
    editor
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

/// Runs `request` to its end, as the host's thread does.
fn run(request: &LineSearchRequest) -> (Vec<LineRow>, LinesEnd) {
    let mut rows = Vec::new();
    let end = request.run(&mut |row| {
        rows.extend(row);
        true
    });
    (rows, end)
}

/// Runs the search the prompt asks for and takes in all it found.
fn search(editor: &mut Editor) {
    let request = editor.take_line_search_request().expect("a search");
    let (rows, end) = run(&request);
    let LinesEnd::Ended(end) = end else {
        panic!("stopped");
    };
    editor.lines_found(request.generation, rows, Some(end));
}

/// The rows shown, in order.
fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect()
}

fn history(editor: &Editor) -> Vec<String> {
    editor
        .histories()
        .filter(|(name, _)| *name == "consult-line")
        .flat_map(|(_, entries)| entries.map(str::to_owned).collect::<Vec<_>>())
        .collect()
}

const TEXT: &str = "one\ntwo\n\nthree\nfour\n";

#[test]
fn every_line_but_the_empty_ones_is_listed_from_the_cursors_round_to_it() {
    // The cursor on `three`.
    let mut editor = searching(TEXT, 10);
    assert_eq!(editor.prompt().unwrap().label(), "Go to line:");
    assert!(editor.searches_project() && editor.files_pending());
    search(&mut editor);
    assert_eq!(rows(&editor), ["4: three", "5: four", "1: one", "2: two"]);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.selected(), Some(0));
    assert!(!editor.files_pending() && !editor.files_cut());
}

#[test]
fn the_line_numbers_are_padded_to_the_widest() {
    let text: String = (1..=12).map(|n| format!("l{n}\n")).collect();
    let mut editor = searching(&text, 0);
    search(&mut editor);
    let shown = rows(&editor);
    assert_eq!(shown[0], " 1: l1");
    assert_eq!(shown[11], "12: l12");
}

#[test]
fn what_is_typed_narrows_in_any_order_and_highlights_after_the_number() {
    let mut editor = searching("let a = 1;\nfn main() {}\nlet main = a;\n", 0);
    search(&mut editor);
    typing(&mut editor, "main let");
    search(&mut editor);
    assert_eq!(rows(&editor), ["3: let main = a;"]);
    let mut out = Vec::new();
    let text = rows(&editor)[0].clone();
    assert_eq!(editor.search_highlights(0, &text, &mut out), Some(3));
    assert_eq!(out, [3..6, 7..11]);
    // `!` leaves lines out, and with nothing else lists the rest.
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(0..end, "!main")]).unwrap();
    editor.match_completion();
    search(&mut editor);
    assert_eq!(rows(&editor), ["1: let a = 1;"]);
}

#[test]
fn one_character_searches() {
    let mut editor = searching(TEXT, 0);
    search(&mut editor);
    typing(&mut editor, "w");
    search(&mut editor);
    assert_eq!(rows(&editor), ["2: two"]);
}

#[test]
fn romaji_matches_its_kana() {
    let mut editor = searching("abc\nかんじ\n", 0);
    search(&mut editor);
    typing(&mut editor, "kan");
    search(&mut editor);
    assert_eq!(rows(&editor), ["2: かんじ"]);
}

#[test]
fn an_input_that_narrows_looks_through_the_lines_found_only() {
    // Only a text this large narrows: a smaller one is looked through again.
    let text = format!("ab\nabc\nx\nabd\n{}", "y\n".repeat(NARROW_PAST / 2));
    let mut editor = searching(&text, 0);
    search(&mut editor);
    typing(&mut editor, "ab");
    search(&mut editor);
    typing(&mut editor, "c");
    let request = editor.take_line_search_request().unwrap();
    assert_eq!(request.narrow.as_deref(), Some(&[0, 1, 3][..]));
    let (found, _) = run(&request);
    editor.lines_found(request.generation, found, Some(WalkEnd { cut: false }));
    assert_eq!(rows(&editor), ["      2: abc"]);
    // Once the buffer changed, the lines found are of another text.
    let buffer = editor.tiled().0.buffer();
    editor.apply_edits(buffer, &[edit(0..0, "abc\n")]).unwrap();
    typing(&mut editor, " !z");
    let request = editor.take_line_search_request().unwrap();
    assert_eq!(request.narrow, None);
    let (found, _) = run(&request);
    assert_eq!(found.len(), 2);
}

#[test]
fn a_small_text_is_looked_through_again() {
    let mut editor = searching("ab\nabc\n", 0);
    search(&mut editor);
    typing(&mut editor, "ab");
    search(&mut editor);
    typing(&mut editor, "c");
    assert_eq!(editor.take_line_search_request().unwrap().narrow, None);
}

#[test]
fn a_search_keeps_at_most_max_rows_and_says_it_was_cut() {
    let text = "x\n".repeat(MAX_ROWS + 5);
    let mut editor = searching(&text, 0);
    let request = editor.take_line_search_request().unwrap();
    let (found, end) = run(&request);
    assert_eq!(found.len(), MAX_ROWS);
    assert_eq!(end, LinesEnd::Ended(WalkEnd { cut: true }));
    editor.lines_found(request.generation, found, Some(WalkEnd { cut: true }));
    assert!(editor.files_cut());
}

#[test]
fn lines_past_a_block_keep_their_numbers_and_a_long_one_shows_once() {
    let mut text = "a\n".repeat(BLOCK);
    text.push_str("needle\n");
    text.push_str(&"needle ".repeat(MAX_LINE / 7 + 10));
    text.push_str("\nneedle\n");
    let mut editor = searching(&text, 0);
    typing(&mut editor, "needle");
    let request = editor.take_line_search_request().unwrap();
    let (found, _) = run(&request);
    let lines: Vec<usize> = found.iter().map(|row| row.line).collect();
    assert_eq!(lines, [BLOCK + 1, BLOCK + 2, BLOCK + 3]);
    assert!(found.iter().all(|row| row.text.len() <= 2 * MAX_ROW_BYTES));
    assert_eq!(found[0].offset, 2 * BLOCK);
}

#[test]
fn a_newer_search_stops_the_one_before() {
    let mut editor = searching(TEXT, 0);
    let first = editor.take_line_search_request().unwrap();
    typing(&mut editor, "t");
    assert_ne!(first.live.load(Ordering::Relaxed), first.generation);
    assert_eq!(run(&first).1, LinesEnd::Stopped);
    let (found, _) = run(&first);
    // The rows of the older one are dropped.
    editor.lines_found(first.generation, found, None);
    assert!(rows(&editor).is_empty());
    // And a taker that says stop stops it.
    let second = editor.take_line_search_request().unwrap();
    assert_eq!(second.run(&mut |_| false), LinesEnd::Stopped);
}

#[test]
fn the_preview_shows_the_buffer_at_the_rows_match() {
    let mut editor = searching("one\nlet needle\n", 0);
    search(&mut editor);
    typing(&mut editor, "needle");
    search(&mut editor);
    let buffer = editor.tiled().0.buffer();
    assert!(matches!(
        editor.preview(),
        Some(Preview::Buffer { id, cursor: 8, .. }) if id == buffer
    ));
    // No file is read for it.
    assert!(editor.take_preview_request().is_none());
    // Once the buffer changed, the start of the line of its number.
    editor.apply_edits(buffer, &[edit(0..0, "zero\n")]).unwrap();
    assert!(matches!(
        editor.preview(),
        Some(Preview::Buffer { cursor: 5, .. })
    ));
}

#[test]
fn ret_goes_to_the_match_with_the_place_before_on_the_mark_ring() {
    let mut editor = searching("one\nlet needle\n", 2);
    search(&mut editor);
    typing(&mut editor, "needle");
    search(&mut editor);
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(text, "2");
    assert!(editor.go_to_line_hit());
    let selections = &editor.window().selections;
    assert_eq!(selections.primary().cursor, 8);
    assert_eq!(selections.primary().mark, Some(2));
    assert_eq!(history(&editor), ["needle"]);
    // Once only.
    assert!(!editor.go_to_line_hit());
}

#[test]
fn c_g_leaves_the_cursor_where_it_was() {
    let mut editor = searching(TEXT, 5);
    search(&mut editor);
    editor.quit();
    assert!(editor.prompt().is_none());
    assert_eq!(editor.window().selections.primary().cursor, 5);
    assert!(!editor.go_to_line_hit());
}

#[test]
fn a_buffer_that_may_hold_secrets_lists_its_lines_but_previews_and_keeps_nothing() {
    let path = PathBuf::from("/w/oso/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(crate::load::decode_all(
        b"KEY=value\nOTHER=x\n",
        Default::default(),
    ));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(30, 100));
    editor.open_line_search("consult-line").unwrap();
    search(&mut editor);
    typing(&mut editor, "OTHER");
    search(&mut editor);
    assert_eq!(rows(&editor), ["2: OTHER=x"]);
    assert_eq!(editor.preview(), Some(Preview::Withheld));
    editor.submit_prompt().unwrap();
    assert!(editor.go_to_line_hit());
    assert!(history(&editor).is_empty());
}

#[test]
fn one_search_at_a_time_and_nothing_of_it_in_debug() {
    let mut editor = searching("secret words\n", 0);
    assert_eq!(
        editor.open_line_search("consult-line"),
        Err(PromptError::Invalid)
    );
    typing(&mut editor, "words");
    let request = editor.take_line_search_request().unwrap();
    let (found, _) = run(&request);
    let debug = format!("{request:?} {found:?}");
    assert!(!debug.contains("secret") && !debug.contains("words"));
}

#[test]
fn an_empty_buffer_lists_nothing() {
    let mut editor = searching("", 0);
    search(&mut editor);
    assert!(rows(&editor).is_empty());
    assert!(!editor.files_pending());
}

#[test]
fn a_narrowed_search_finds_a_match_past_the_first_piece_of_a_long_line() {
    let mut text = "x".repeat(MAX_LINE + 100);
    text.push_str("foob\n");
    let mut editor = searching(&text, 0);
    search(&mut editor);
    typing(&mut editor, "foo");
    search(&mut editor);
    assert_eq!(editor.project_search.places.len(), 1);
    typing(&mut editor, "b");
    let request = editor.take_line_search_request().unwrap();
    assert!(request.narrow.is_some());
    let (found, _) = run(&request);
    assert_eq!(found.iter().map(|row| row.line).collect::<Vec<_>>(), [1]);
    assert_eq!(found[0].offset, MAX_LINE + 100);
}

#[test]
fn a_match_inside_a_character_previews_and_goes_to_its_start() {
    let mut editor = searching("é\n", 0);
    search(&mut editor);
    typing(&mut editor, "(?-u:\\xA9)");
    search(&mut editor);
    assert_eq!(rows(&editor).len(), 1);
    assert!(matches!(
        editor.preview(),
        Some(Preview::Buffer { cursor: 0, .. })
    ));
    editor.submit_prompt().unwrap();
    assert!(editor.go_to_line_hit());
    assert_eq!(editor.window().selections.primary().cursor, 0);
}

#[test]
fn ret_after_the_buffer_changed_goes_to_the_line_of_the_rows_number() {
    let mut editor = searching("one\nlet needle\n", 0);
    search(&mut editor);
    typing(&mut editor, "needle");
    search(&mut editor);
    let buffer = editor.tiled().0.buffer();
    editor.apply_edits(buffer, &[edit(0..0, "zero\n")]).unwrap();
    editor.submit_prompt().unwrap();
    assert!(editor.go_to_line_hit());
    assert_eq!(editor.window().selections.primary().cursor, 5);
}

#[test]
fn ret_once_the_buffer_has_gone_takes_nothing_of_its_text() {
    let mut editor = Editor::with_buffer(Buffer::from_text("other", "x\n"));
    editor.set_view_size(ViewSize::new(30, 100));
    let id = editor.add_buffer(Buffer::from_text("t", "secretive needle\n"));
    editor.show(id);
    editor.open_line_search("consult-line").unwrap();
    search(&mut editor);
    editor.kill_buffer(id);
    if editor.prompt().is_some() {
        let _ = editor.submit_prompt();
    }
    assert!(!editor.go_to_line_hit());
    assert!(
        history(&editor)
            .iter()
            .all(|entry| !entry.contains("needle"))
    );
}

#[test]
fn crlf_lines_keep_their_numbers_across_blocks_and_no_block_ends_inside_a_break() {
    let mut text = "a\r\n".repeat(BLOCK / 3 + 7);
    text.push_str("needle\r\n");
    let mut editor = searching(&text, 0);
    typing(&mut editor, "needle");
    let request = editor.take_line_search_request().unwrap();
    let (found, _) = run(&request);
    assert_eq!(
        found.iter().map(|row| row.line).collect::<Vec<_>>(),
        [BLOCK / 3 + 8]
    );
    // `^$` lists only the empty line after the last break: a block never ends between `\r`
    // and `\n`, which would make an empty line of its own.
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(0..end, "^$")]).unwrap();
    editor.match_completion();
    let request = editor.take_line_search_request().unwrap();
    let (found, _) = run(&request);
    let found: Vec<(usize, usize)> = found.iter().map(|row| (row.line, row.offset)).collect();
    assert_eq!(found, [(BLOCK / 3 + 9, text.len())]);
}
