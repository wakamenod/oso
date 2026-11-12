//! Tests of a project's search (ADR 0114): when a search starts, rows taken in as they come,
//! stale searches, narrowing, what `RET` gives and the place it goes to.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use super::{ProjectSearchHit, ProjectSearchRow, WITHHELD};
use crate::buffer::Buffer;
use crate::editor::edit_tests::edit;
use crate::editor::{Editor, NotInProject, WalkEnd};
use crate::minibuffer::Outcome;
use crate::project::MAX_ROWS;

/// An editor with a search of `/w/oso` open.
fn searching() -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
    editor
        .open_project_search(PathBuf::from("/w/oso"), "oso", "project-search")
        .unwrap();
    editor
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

fn row(path: &str, line: usize, text: &str) -> ProjectSearchRow {
    ProjectSearchRow {
        path: path.into(),
        line,
        column: 4,
        text: text.into(),
        secret: false,
    }
}

/// The rows shown, in order.
fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect()
}

/// Types `text` and gives the generation of the search it starts.
fn search_for(editor: &mut Editor, text: &str) -> u64 {
    typing(editor, text);
    editor.take_search_request().expect("a search").generation
}

#[test]
fn a_search_starts_once_a_component_has_two_characters() {
    let mut editor = searching();
    assert_eq!(editor.prompt().unwrap().label(), "Search oso:");
    assert!(editor.take_search_request().is_none());
    typing(&mut editor, "!no a");
    assert!(editor.take_search_request().is_none());
    assert!(!editor.files_pending());
    typing(&mut editor, "b");
    let request = editor.take_search_request().unwrap();
    assert_eq!(request.root, PathBuf::from("/w/oso"));
    assert_eq!(request.live.load(Ordering::Relaxed), request.generation);
    assert_eq!(request.narrow, None);
    assert!(editor.files_pending(), "the count shows … while it runs");
    let debug = format!("{request:?}");
    assert!(!debug.contains("ab") && !debug.contains("oso"));
    // The same input again starts nothing.
    editor.match_completion();
    assert!(editor.take_search_request().is_none());
}

#[test]
fn rows_go_after_those_shown_and_the_selected_row_stays() {
    let mut editor = searching();
    let generation = search_for(&mut editor, "needle");
    editor.rows_found(generation, vec![row("b.rs", 3, "needle()")], None);
    editor.rows_found(generation, vec![row("a.rs", 1, "let needle")], None);
    assert_eq!(rows(&editor), ["b.rs:3: needle()", "a.rs:1: let needle"]);
    editor.move_completion(1).unwrap();
    editor.rows_found(generation, vec![row("a.rs", 9, "needle")], None);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.selected(), Some(1));
    assert!(editor.files_pending());
    editor.rows_found(generation, Vec::new(), Some(WalkEnd { cut: false }));
    assert!(!editor.files_pending() && !editor.files_cut());
}

#[test]
fn a_newer_input_drops_the_rows_and_a_stale_searchs_rows_are_dropped() {
    let mut editor = searching();
    let first = search_for(&mut editor, "ne");
    editor.rows_found(first, vec![row("a.rs", 1, "ne")], None);
    let request = {
        typing(&mut editor, "x");
        editor.take_search_request().unwrap()
    };
    assert_ne!(
        request.live.load(Ordering::Relaxed),
        first,
        "the first stops"
    );
    assert!(rows(&editor).is_empty());
    editor.rows_found(first, vec![row("b.rs", 1, "ne")], None);
    assert!(rows(&editor).is_empty());
    // Back under two characters: no search, and no rows.
    let id = editor.prompt().unwrap().buffer();
    editor.apply_edits(id, &[edit(0..3, "n")]).unwrap();
    editor.match_completion();
    assert!(editor.take_search_request().is_none());
    assert!(!editor.files_pending());
}

#[test]
fn an_input_that_narrows_a_whole_search_looks_through_its_lines() {
    let mut editor = searching();
    let first = search_for(&mut editor, "foo");
    editor.rows_found(
        first,
        vec![row("a.rs", 1, "foo")],
        Some(WalkEnd { cut: false }),
    );
    typing(&mut editor, "b");
    assert_eq!(editor.take_search_request().unwrap().narrow, Some(first));
    // The last whole search stands while a later one runs.
    typing(&mut editor, "a");
    assert_eq!(editor.take_search_request().unwrap().narrow, Some(first));
    // An input that does not narrow it walks again, and a cut search is never whole.
    let id = editor.prompt().unwrap().buffer();
    editor.apply_edits(id, &[edit(0..5, "zz")]).unwrap();
    editor.match_completion();
    let cut = editor.take_search_request().unwrap();
    assert_eq!(cut.narrow, None);
    editor.rows_found(cut.generation, Vec::new(), Some(WalkEnd { cut: true }));
    assert!(editor.files_cut(), "the count says +");
    typing(&mut editor, "z");
    assert_eq!(editor.take_search_request().unwrap().narrow, None);
}

#[test]
fn a_search_keeps_at_most_its_rows() {
    let mut editor = searching();
    let generation = search_for(&mut editor, "xx");
    let many: Vec<_> = (0..MAX_ROWS + 5)
        .map(|line| row("a.rs", line, "x"))
        .collect();
    let live = editor.project_search.live.load(Ordering::Relaxed);
    editor.rows_found(generation, many, None);
    assert_eq!(rows(&editor).len(), MAX_ROWS);
    assert!(editor.files_cut() && !editor.files_pending());
    assert_ne!(editor.project_search.live.load(Ordering::Relaxed), live);
}

#[test]
fn a_secret_files_row_shows_its_place_and_no_text() {
    let mut editor = searching();
    let generation = search_for(&mut editor, "token");
    let secret = ProjectSearchRow {
        path: ".env".into(),
        line: 2,
        column: 0,
        text: "".into(),
        secret: true,
    };
    editor.rows_found(
        generation,
        vec![secret, row("a.rs", 1, "a token here")],
        None,
    );
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    let first = candidates.get(0).unwrap();
    assert_eq!((first.text(), first.annotation()), (".env:2", WITHHELD));
    let mut out = Vec::new();
    assert_eq!(editor.search_highlights(0, ".env:2", &mut out), Some(6));
    assert!(out.is_empty());
    let shown = candidates.get(1).unwrap().text().to_owned();
    assert_eq!(editor.search_highlights(1, &shown, &mut out), Some(8));
    assert_eq!((out.len(), out[0].clone()), (1, 10..15));
    assert_eq!(&shown[10..15], "token");
}

#[test]
fn ret_gives_the_files_whole_path_its_place_and_keeps_the_input() {
    let mut editor = searching();
    let generation = search_for(&mut editor, "needle");
    editor.rows_found(generation, vec![row("src/a.rs", 7, "x needle")], None);
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(PathBuf::from(text), PathBuf::from("/w/oso/src/a.rs"));
    assert_eq!(
        editor.take_search_hit(),
        Some(ProjectSearchHit {
            path: PathBuf::from("/w/oso/src/a.rs"),
            line: 7,
            column: 4,
        })
    );
    let histories: Vec<(String, Vec<String>)> = editor
        .histories()
        .map(|(name, entries)| (name.to_owned(), entries.map(str::to_owned).collect()))
        .collect();
    assert_eq!(
        histories,
        [("project-search".to_owned(), vec!["needle".to_owned()])]
    );
}

#[test]
fn closing_the_search_stops_it_and_a_second_is_refused_while_one_is_open() {
    let mut editor = searching();
    assert!(
        editor
            .open_project_search(PathBuf::from("/w/oso"), "oso", "project-search")
            .is_err()
    );
    let generation = search_for(&mut editor, "needle");
    let live = std::sync::Arc::clone(&editor.project_search.live);
    editor.quit();
    assert_ne!(live.load(Ordering::Relaxed), generation);
    editor.rows_found(generation, vec![row("a.rs", 1, "needle")], None);
    assert!(editor.prompt().is_none());
}

#[test]
fn a_named_directory_stands_for_a_project_but_not_home_or_what_holds_it() {
    let mut editor = searching();
    editor.set_home(Some(PathBuf::from("/Users/me")));
    assert_eq!(
        editor.listable_root(std::path::Path::new("/Users/me/src/oso")),
        Ok((PathBuf::from("/Users/me/src/oso"), "oso".to_owned()))
    );
    for refused in [
        "/Users/me",
        "/Users",
        "/",
        "/Users/me/..",
        "/..",
        "/Users/me/src/../..",
    ] {
        assert_eq!(
            editor.listable_root(std::path::Path::new(refused)),
            Err(NotInProject),
            "{refused}"
        );
    }
}

#[test]
fn the_search_splits_its_input_as_completion_does() {
    use crate::complete::{Candidate, Query};
    // `project` keeps its own copy of completion's split, as the two share a layer.
    let texts = ["a b", "ab", "b a", "x a b y", "a\\b", "日本 語"];
    for input in ["a b", "a\\ b", "ab", "  a  ", "b a", "日本 語", "a !b"] {
        for text in texts {
            let completes = Query::parse(input).matches(&Candidate::new(text, "", ""));
            let searches = crate::project::LineQuery::parse(input).matches_line(text.as_bytes());
            assert_eq!(completes, searches.is_some(), "{input:?} over {text:?}");
        }
    }
    let long = "a".repeat(crate::complete::MAX_INPUT_BYTES + 1);
    assert!(!crate::project::LineQuery::parse(&long).starts());
}

#[test]
fn tab_leaves_what_is_looked_for_as_it_is() {
    let mut editor = searching();
    let generation = search_for(&mut editor, "needle");
    editor.rows_found(generation, vec![row("a.rs", 1, "let needle")], None);
    editor.insert_completion().unwrap();
    let id = editor.prompt().unwrap().buffer();
    assert_eq!(editor.buffer(id).unwrap().text().to_string(), "needle");
    assert!(editor.take_search_request().is_none());
    assert_eq!(rows(&editor), ["a.rs:1: let needle"]);
}

#[test]
#[ignore = "a timing: run by hand in a release build"]
fn the_ui_threads_share_of_a_batch_of_rows() {
    use std::time::{Duration, Instant};
    let text = format!(
        "    let ÜBER = {}needle(x, y);",
        "Some_Mixed_Case_".repeat(30)
    );
    let mut slowest = [Duration::ZERO; 3];
    for (at, size) in [1000, 5000, 10_000].into_iter().enumerate() {
        for _ in 0..5 {
            let mut editor = searching();
            let generation = search_for(&mut editor, "needle");
            let batch: Vec<_> = (0..size)
                .map(|line| row("src/some/deeper/path/file.rs", line, &text))
                .collect();
            let started = Instant::now();
            editor.rows_found(generation, batch, None);
            slowest[at] = slowest[at].max(started.elapsed());
        }
    }
    assert!(slowest[0] < Duration::from_millis(1), "{slowest:?}");
}

/// The project-search histories the editor keeps.
fn search_history(editor: &Editor) -> Vec<String> {
    editor
        .histories()
        .filter(|(name, _)| *name == "project-search")
        .flat_map(|(_, entries)| entries.map(str::to_owned).collect::<Vec<_>>())
        .collect()
}

#[test]
fn a_secret_files_row_keeps_no_history_of_what_found_it() {
    let mut editor = searching();
    let generation = search_for(&mut editor, "token");
    let secret = ProjectSearchRow {
        path: ".env".into(),
        line: 1,
        column: 0,
        text: "".into(),
        secret: true,
    };
    editor.rows_found(generation, vec![secret], None);
    editor.submit_prompt().unwrap();
    assert!(matches!(
        editor.take_prompt_outcome(),
        Some(Outcome::Answered { .. })
    ));
    assert_eq!(editor.take_search_hit().map(|hit| hit.line), Some(1));
    assert!(search_history(&editor).is_empty());
}

#[test]
fn a_search_beside_a_buffer_that_may_hold_secrets_keeps_no_history() {
    let path = PathBuf::from("/w/oso/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(crate::load::decode_all(b"KEY=value\n", Default::default()));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    let mut editor = Editor::with_buffer(buffer);
    editor
        .open_project_search(PathBuf::from("/w/oso"), "oso", "project-search")
        .unwrap();
    let generation = search_for(&mut editor, "needle");
    editor.rows_found(generation, vec![row("a.rs", 1, "needle")], None);
    editor.submit_prompt().unwrap();
    assert!(search_history(&editor).is_empty());
}
