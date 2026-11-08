//! Tests of the prompt that reads a path, with listings made up.
//!
//! Must not depend on: threads or the file system.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::edit_tests::edit;
use crate::complete::path::{Name, NameKind};
use crate::editor::Editor;
use crate::minibuffer::Outcome;

fn names(entries: &[(&str, NameKind)]) -> Option<Arc<[Name]>> {
    Some(
        entries
            .iter()
            .map(|(name, kind)| Name {
                name: (*name).to_owned(),
                kind: *kind,
            })
            .collect(),
    )
}

/// An editor with a prompt for a path open at `~/`, home being `/home/me`, its listing given.
fn prompt() -> Editor {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_file_prompt("Find file:", "file", "~/", PathBuf::from("/work"))
        .unwrap();
    assert_eq!(editor.take_file_requests(), [PathBuf::from("/home/me")]);
    assert!(editor.files_pending());
    use NameKind::{Directory as D, File as F};
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("notes", D), ("todo.md", F), (".config", D)]),
        false,
    );
    editor
}

fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    candidates
        .shown(18)
        .map(|(_, candidate)| candidate.text().to_owned())
        .collect()
}

fn answer(editor: &Editor) -> String {
    let id = editor.prompt().unwrap().buffer();
    editor.buffer(id).unwrap().text().to_string()
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

#[test]
fn a_listing_fills_the_list_without_hidden_names() {
    let editor = prompt();
    assert!(!editor.files_pending());
    assert_eq!(rows(&editor), ["notes/", "todo.md"]);
    assert_eq!(editor.file_directory(), Some(Path::new("/home/me")));
}

#[test]
fn the_last_segment_is_matched_and_a_new_directory_is_asked_for() {
    let mut editor = prompt();
    typing(&mut editor, "do");
    assert_eq!(rows(&editor), ["todo.md"], "the matcher over names only");
    typing(&mut editor, "/");
    assert_eq!(
        editor.take_file_requests(),
        Vec::<PathBuf>::new(),
        "no todo.md/ to list"
    );
}

#[test]
fn tab_puts_the_whole_path_in_and_ret_on_a_directory_goes_into_it() {
    let mut editor = prompt();
    editor.insert_completion().unwrap();
    assert_eq!(answer(&editor), "~/notes/");
    assert_eq!(
        editor.take_file_requests(),
        [PathBuf::from("/home/me/notes")]
    );
    editor.file_listed(
        PathBuf::from("/home/me/notes"),
        names(&[("plan.md", NameKind::File)]),
        false,
    );
    assert_eq!(rows(&editor), ["plan.md"]);
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(text, "/home/me/notes/plan.md");
}

#[test]
fn ret_on_a_directory_goes_into_it_and_stays_open() {
    let mut editor = prompt();
    editor.submit_prompt().unwrap();
    assert!(editor.prompt().is_some());
    assert_eq!(answer(&editor), "~/notes/");
}

#[test]
fn a_path_that_matches_nothing_is_given_as_typed() {
    let mut editor = prompt();
    typing(&mut editor, "new.md");
    assert!(rows(&editor).is_empty());
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(text, "/home/me/new.md");
}

#[test]
fn m_ret_gives_what_is_typed_while_a_name_matches() {
    let mut editor = prompt();
    typing(&mut editor, "to");
    editor.submit_typed().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(text, "/home/me/to");
}

#[test]
fn a_directory_that_cannot_be_read_says_so_and_lists_nothing() {
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/locked/", PathBuf::from("/work"))
        .unwrap();
    assert_eq!(editor.take_file_requests(), [PathBuf::from("/")]);
    editor.file_listed(
        PathBuf::from("/"),
        names(&[("locked", NameKind::Directory)]),
        false,
    );
    assert_eq!(editor.take_file_requests(), [PathBuf::from("/locked")]);
    editor.file_listed(PathBuf::from("/locked"), None, false);
    assert!(rows(&editor).is_empty());
    assert_eq!(editor.prompt().unwrap().message(), Some("Cannot list it"));
}

#[test]
fn a_listing_after_the_prompt_closed_changes_nothing() {
    let mut editor = prompt();
    editor.quit();
    editor.file_listed(PathBuf::from("/home/me/notes"), names(&[]), false);
    assert!(editor.prompt().is_none());
    assert_eq!(editor.take_file_requests(), Vec::<PathBuf>::new());
}

/// Answers every listing the prompt asks for from `tree`, a listing cut short where `cut`.
fn serve(editor: &mut Editor, tree: &[(&str, &[(&str, NameKind)])], cut: bool) {
    for _ in 0..8 {
        let asked = editor.take_file_requests();
        if asked.is_empty() {
            return;
        }
        for directory in asked {
            let listed = tree
                .iter()
                .find(|(path, _)| Path::new(path) == directory)
                .and_then(|(_, entries)| names(entries));
            editor.file_listed(directory, listed, cut);
        }
    }
}

#[test]
fn only_the_name_is_matched_beside_the_directories_that_tell_rows_apart() {
    use NameKind::{Directory as D, File as F};
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/h/D/", PathBuf::from("/"))
        .unwrap();
    let tree: &[(&str, &[(&str, NameKind)])] = &[
        ("/", &[("h", D)]),
        ("/h", &[("Desktop", D), ("Documents", D)]),
        ("/h/Desktop", &[("a.txt", F)]),
        ("/h/Documents", &[("b.md", F)]),
    ];
    serve(&mut editor, tree, false);
    assert_eq!(rows(&editor), ["b.md", "a.txt"], "shorter first");
    typing(&mut editor, "t");
    assert_eq!(
        rows(&editor),
        ["a.txt"],
        "not every row for the t of Desktop"
    );
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.get(0).unwrap().annotation(), "Desktop/");
}

#[test]
fn a_listing_no_plan_waits_for_keeps_the_selection_where_it_is() {
    let mut editor = prompt();
    editor.move_completion(1).unwrap();
    editor.file_listed(PathBuf::from("/elsewhere"), names(&[]), false);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(
        candidates.selected_candidate().map(|c| c.text()),
        Some("todo.md")
    );
}

#[test]
fn a_question_over_the_prompt_keeps_it_open_for_its_listings() {
    let mut editor = prompt();
    editor.ask("Go on?", "keyboard-quit").unwrap();
    assert!(editor.file_prompt_open());
    assert!(!editor.files_pending());
}

#[test]
fn a_listing_cut_short_shows_in_the_count() {
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/big/", PathBuf::from("/"))
        .unwrap();
    let tree: &[(&str, &[(&str, NameKind)])] = &[
        ("/", &[("big", NameKind::Directory)]),
        ("/big", &[("a", NameKind::File)]),
    ];
    serve(&mut editor, tree, true);
    assert!(editor.files_cut());
}
