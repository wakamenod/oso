//! Tests of a prompt for a directory (ADR 0114): only directories and links listed, what is
//! typed given as the listings allow, and `RET` and `M-RET` waiting for the listing or the look
//! that decides the answer.
//!
//! Must not depend on: threads or the file system.

use std::path::{Path, PathBuf};

use super::super::tests::{answer, looked, names, rows, typing};
use crate::complete::path::NameKind;
use crate::editor::Editor;
use crate::editor::edit_tests::edit;
use crate::minibuffer::Outcome;

#[test]
fn a_directory_prompt_lists_no_file_and_gives_a_directory_typed_with_a_slash() {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    use NameKind::{Directory as D, File as F, Link as L, Other as O};
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("notes", D), ("todo.md", F), ("ln", L), ("pipe", O)]),
        false,
    );
    // Only directories and links, which may lead to one, as `read-directory-name` lists.
    assert_eq!(rows(&editor), ["ln", "notes/"]);
    // What is typed with `/` at its end is the directory, whatever row is selected.
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(0..end, "~/notes/")]).unwrap();
    editor.match_completion();
    editor.take_file_requests();
    editor.file_listed(
        PathBuf::from("/home/me/notes"),
        names(&[("a.md", F)]),
        false,
    );
    assert!(rows(&editor).is_empty(), "a directory of files lists none");
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    // A path of the system's, `/` at its end saying it is a directory.
    assert_eq!(PathBuf::from(&text), Path::new("/home/me").join("notes"));
    assert!(text.ends_with('/'));
}

/// A prompt for a directory at `~/`, home being `/home/me`, its listing given.
fn directory_prompt() -> Editor {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    use NameKind::{Directory as D, File as F, Link as L};
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("notes", D), ("todo.md", F), ("ln", L)]),
        false,
    );
    editor
}

fn message(editor: &Editor) -> Option<&str> {
    editor.prompt().and_then(|prompt| prompt.message())
}

#[test]
fn a_directory_prompt_goes_into_a_directorys_row() {
    let mut editor = directory_prompt();
    typing(&mut editor, "not");
    editor.submit_prompt().unwrap();
    assert!(editor.prompt().is_some(), "still open");
    assert_eq!(answer(&editor), "~/notes/");
}

#[test]
fn a_directory_prompt_refuses_a_file_typed_whole_and_a_name_its_listing_lacks() {
    for typed in [true, false] {
        let mut editor = directory_prompt();
        typing(&mut editor, "todo.md");
        let submitted = if typed {
            editor.submit_typed()
        } else {
            // No row matches, so what is typed is taken.
            editor.submit_prompt()
        };
        assert!(submitted.is_err(), "typed: {typed}");
        assert_eq!(message(&editor), Some("Not a directory: ~/todo.md"));
    }
    let mut editor = directory_prompt();
    typing(&mut editor, "elsewhere");
    assert!(editor.submit_typed().is_err());
    assert_eq!(message(&editor), Some("No such directory: ~/elsewhere"));
    assert!(editor.prompt().is_some(), "it stays open");
    // A directory typed whole is given.
    let mut editor = directory_prompt();
    typing(&mut editor, "notes");
    editor.submit_typed().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(PathBuf::from(&text), Path::new("/home/me").join("notes"));
}

#[test]
fn a_directory_prompt_waits_for_the_listing_that_decides_its_answer() {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    // Typed before `~/` is listed: `RET` and `M-RET` wait, as the host holds them.
    typing(&mut editor, "todo.md");
    assert!(editor.directory_answer_waits());
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("todo.md", NameKind::File)]),
        false,
    );
    assert!(!editor.directory_answer_waits());
    assert!(editor.submit_typed().is_err());
    assert_eq!(message(&editor), Some("Not a directory: ~/todo.md"));
    // A prompt for a file never waits so.
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/r/", PathBuf::from("/"))
        .unwrap();
    editor.take_file_requests();
    assert!(!editor.directory_answer_waits());
}

#[test]
fn a_directory_prompt_says_a_directory_it_cannot_read() {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    editor.file_listed(PathBuf::from("/home/me"), None, false);
    typing(&mut editor, "x");
    assert!(editor.submit_typed().is_err());
    assert_eq!(message(&editor), Some("Cannot read directory: ~/"));
    let mut editor = directory_prompt();
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(0..end, "~/notes/")]).unwrap();
    editor.match_completion();
    editor.take_file_requests();
    editor.file_listed(PathBuf::from("/home/me/notes"), None, false);
    assert!(editor.submit_prompt().is_err());
    assert_eq!(message(&editor), Some("Cannot read directory: ~/notes/"));
}

#[test]
fn a_directory_prompt_takes_a_link_a_look_found_leads_to_a_directory() {
    // The link selected waits for its look, before it is asked for as after.
    let mut editor = directory_prompt();
    typing(&mut editor, "ln");
    assert!(editor.directory_answer_waits(), "not asked yet");
    editor.take_file_looks();
    assert!(editor.directory_answer_waits());
    editor.file_looked(
        Path::new("/home/me"),
        vec![("ln".to_owned(), looked("l", 0, false))],
    );
    assert!(!editor.directory_answer_waits());
    assert!(editor.submit_prompt().is_err());
    assert_eq!(message(&editor), Some("Not a directory: ~/ln"));
    assert!(editor.submit_typed().is_err(), "typed whole too");
    // One a look found leads to a directory is gone into, or given typed whole.
    let mut editor = directory_prompt();
    typing(&mut editor, "ln");
    editor.take_file_looks();
    editor.file_looked(
        Path::new("/home/me"),
        vec![("ln".to_owned(), looked("l", 0, true))],
    );
    editor.submit_typed().unwrap();
    assert!(editor.prompt().is_none());
}

#[test]
fn a_name_a_look_in_its_directory_left_out_is_asked_for_again() {
    // The host keeps one look waiting per directory, so a newer one may take an older's place.
    let mut editor = directory_prompt();
    typing(&mut editor, "ln");
    assert!(!editor.take_file_looks().is_empty());
    editor.file_looked(
        Path::new("/home/me"),
        vec![("notes".to_owned(), looked("d", 0, true))],
    );
    assert!(editor.directory_answer_waits());
    let again = editor.take_file_looks();
    assert!(
        again.contains(&(PathBuf::from("/home/me"), vec!["ln".to_owned()])),
        "{again:?}"
    );
    editor.file_looked(
        Path::new("/home/me"),
        vec![("ln".to_owned(), looked("l", 0, true))],
    );
    assert!(!editor.directory_answer_waits());
}

#[test]
fn the_row_selected_is_looked_at_though_the_float_shows_no_rows() {
    let mut editor = directory_prompt();
    editor.set_frame_size(crate::frame::FrameSize {
        rows: 2,
        columns: 40,
    });
    assert_eq!(editor.candidate_rows(), 0, "no room for a row");
    while editor
        .selected_found()
        .is_some_and(|found| found.path != Path::new("/home/me/ln"))
    {
        editor.move_completion(1).unwrap();
    }
    assert!(editor.directory_answer_waits());
    let looks = editor.take_file_looks();
    assert_eq!(looks, [(PathBuf::from("/home/me"), vec!["ln".to_owned()])]);
    editor.file_looked(
        Path::new("/home/me"),
        vec![("ln".to_owned(), looked("l", 0, true))],
    );
    assert!(!editor.directory_answer_waits());
}

#[test]
fn a_link_typed_whole_waits_for_a_look_of_its_own() {
    let mut editor = directory_prompt();
    // `M-RET` answers `~/ln` as typed, which its look decides, whatever row is selected.
    typing(&mut editor, "ln");
    assert_eq!(
        editor.typed_link_unlooked(),
        Some(PathBuf::from("/home/me/ln"))
    );
    assert!(editor.directory_answer_waits());
    let looks = editor.take_file_looks();
    assert!(
        looks.contains(&(PathBuf::from("/home/me"), vec!["ln".to_owned()])),
        "{looks:?}"
    );
    assert!(editor.take_file_looks().is_empty(), "asked once");
    editor.file_looked(
        Path::new("/home/me"),
        vec![("ln".to_owned(), looked("l", 0, false))],
    );
    assert!(!editor.directory_answer_waits());
    assert!(editor.submit_typed().is_err());
    assert_eq!(message(&editor), Some("Not a directory: ~/ln"));
    // Typed with `/` at its end, its own listing decides instead.
    let mut editor = directory_prompt();
    typing(&mut editor, "ln/");
    assert!(editor.typed_link_unlooked().is_none());
}
