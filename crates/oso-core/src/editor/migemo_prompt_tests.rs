//! Tests of migemo in a project's search and in completion on the editor (ADR 0116): the
//! dictionary taken for a search's or a prompt's migemo components, the search started again
//! or the input matched again when one comes, when either wants one, and the setting. Every
//! dictionary here is synthetic.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use std::path::PathBuf;
use std::sync::Arc;

use super::edit_tests::edit;
use super::test_buffers::secret_buffer;
use super::{Editor, Quit, WalkEnd};
use crate::buffer::Buffer;
use crate::complete::{Candidate, WORKER_PAST};
use crate::migemo::Dictionary;
use crate::minibuffer::Kind;

fn dictionary() -> Option<Arc<Dictionary>> {
    Some(Arc::new(Dictionary::new("かんじ\t漢字\n".to_owned())))
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

/// An editor with a search of `/w/oso` open, over `buffer`.
fn searching(buffer: Buffer) -> Editor {
    let mut editor = Editor::with_buffer(buffer);
    editor
        .open_project_search(PathBuf::from("/w/oso"), "oso", "project-search")
        .unwrap();
    editor
}

#[test]
fn a_project_search_takes_the_dictionary_and_searches_again_when_it_comes() {
    let mut editor = searching(Buffer::from_text("t", ""));
    typing(&mut editor, "kanji");
    let first = editor.take_search_request().unwrap();
    assert!(first.query.matches_line("かんじ".as_bytes()).is_some());
    assert!(first.query.matches_line("漢字".as_bytes()).is_none());
    assert!(editor.take_migemo_dictionary_wanted());
    // Ended whole, so only new words keep the next search from narrowing it.
    editor.rows_found(first.generation, Vec::new(), Some(WalkEnd { cut: false }));
    editor.set_migemo_dictionary(dictionary());
    let again = editor.take_search_request().expect("searched again");
    assert!(again.generation > first.generation);
    assert!(again.query.matches_line("漢字".as_bytes()).is_some());
    assert_eq!(again.narrow, None, "its words are new");
    assert!(!editor.take_migemo_dictionary_wanted());
    // A key keeps the dictionary.
    typing(&mut editor, " x1");
    let next = editor.take_search_request().unwrap();
    assert!(next.query.matches_line("漢字 x1".as_bytes()).is_some());
}

/// Opens a prompt over the innermost one.
fn nested(editor: &mut Editor) {
    let kind = Kind::Number { history: None };
    editor.open_prompt("Go to line: ", kind).unwrap();
}

#[test]
fn a_project_search_under_a_prompt_searches_again_once_that_closes() {
    let mut editor = searching(Buffer::from_text("t", ""));
    typing(&mut editor, "kanji");
    let first = editor.take_search_request().unwrap();
    // A prompt closing over it with no new dictionary leaves it as it is.
    nested(&mut editor);
    assert_eq!(editor.quit(), Quit::Prompt);
    assert!(editor.take_search_request().is_none());
    nested(&mut editor);
    editor.set_migemo_dictionary(dictionary());
    assert!(editor.take_search_request().is_none(), "covered");
    assert_eq!(editor.quit(), Quit::Prompt);
    let again = editor.take_search_request().expect("searched again");
    assert!(again.generation > first.generation);
    assert!(again.query.matches_line("漢字".as_bytes()).is_some());
}

#[test]
fn a_project_search_with_no_migemo_component_is_left_as_it_is() {
    let mut editor = searching(Buffer::from_text("t", ""));
    typing(&mut editor, "Struct");
    assert!(editor.take_search_request().is_some());
    assert!(!editor.take_migemo_dictionary_wanted());
    editor.set_migemo_dictionary(dictionary());
    assert!(editor.take_search_request().is_none());
}

#[test]
fn the_setting_off_keeps_a_project_search_literal() {
    let mut editor = searching(Buffer::from_text("t", ""));
    editor.set_migemo(false);
    typing(&mut editor, "kanji");
    let request = editor.take_search_request().unwrap();
    assert!(request.query.matches_line("かんじ".as_bytes()).is_none());
    assert!(request.query.matches_line(b"kanji").is_some());
    assert!(!editor.take_migemo_dictionary_wanted());
}

#[test]
fn around_a_secret_a_project_search_wants_no_dictionary() {
    let mut editor = searching(secret_buffer());
    typing(&mut editor, "kanji");
    let request = editor.take_search_request().unwrap();
    assert!(request.query.matches_line("かんじ".as_bytes()).is_some());
    assert!(!editor.take_migemo_dictionary_wanted());
}

/// An editor with a prompt completing `texts` open, over `buffer`.
fn completing(buffer: Buffer, texts: &[String]) -> Editor {
    let mut editor = Editor::with_buffer(buffer);
    let items: Arc<[Candidate]> = texts
        .iter()
        .map(|text| Candidate::new(&**text, "", ""))
        .collect();
    editor.open_completion("Find:", "find", items).unwrap();
    editor
}

fn shown(editor: &Editor) -> usize {
    editor.prompt().unwrap().candidates().unwrap().len()
}

fn names() -> Vec<String> {
    ["漢字.md", "かんじ.md", "other.md"]
        .map(str::to_owned)
        .to_vec()
}

#[test]
fn a_completion_matches_again_when_the_dictionary_comes() {
    let mut editor = completing(Buffer::from_text("t", ""), &names());
    typing(&mut editor, "%kan");
    assert_eq!(shown(&editor), 1);
    assert!(editor.take_migemo_dictionary_wanted());
    editor.set_migemo_dictionary(dictionary());
    assert_eq!(shown(&editor), 2);
    // And when it goes, matches without it.
    editor.set_migemo_dictionary(None);
    assert_eq!(shown(&editor), 1);
    // Without `%`, a component wants none.
    let mut editor = completing(Buffer::from_text("t", ""), &names());
    typing(&mut editor, "kan");
    assert!(!editor.take_migemo_dictionary_wanted());
}

#[test]
fn a_completion_under_a_prompt_matches_again_once_that_closes() {
    let mut editor = completing(Buffer::from_text("t", ""), &names());
    typing(&mut editor, "%kan");
    assert_eq!(shown(&editor), 1);
    nested(&mut editor);
    editor.set_migemo_dictionary(dictionary());
    assert_eq!(editor.quit(), Quit::Prompt);
    assert_eq!(shown(&editor), 2);
    nested(&mut editor);
    editor.set_migemo_dictionary(None);
    assert_eq!(editor.quit(), Quit::Prompt);
    assert_eq!(shown(&editor), 1);
}

#[test]
fn around_a_secret_a_completion_wants_no_dictionary() {
    let mut editor = completing(secret_buffer(), &names());
    typing(&mut editor, "%kan");
    assert_eq!(shown(&editor), 1);
    assert!(!editor.take_migemo_dictionary_wanted());
}

/// Runs every job left, as the worker would, and hands each report back.
fn work(editor: &mut Editor) {
    for _ in 0..8 {
        let Some(job) = editor.take_completion_job() else {
            return;
        };
        if let Some(report) = job.run() {
            editor.completion_reported(report);
        }
    }
}

#[test]
fn a_completion_on_the_worker_takes_the_dictionary_and_matches_again_when_it_comes() {
    let mut texts = names();
    texts.extend((0..WORKER_PAST).map(|index| format!("file-{index:04}.txt")));
    let mut editor = completing(Buffer::from_text("t", ""), &texts);
    typing(&mut editor, "%kan");
    work(&mut editor);
    assert_eq!(shown(&editor), 1);
    assert!(editor.take_migemo_dictionary_wanted());
    editor.set_migemo_dictionary(dictionary());
    work(&mut editor);
    assert_eq!(shown(&editor), 2);
    // A key goes to the worker with it.
    typing(&mut editor, " .md");
    work(&mut editor);
    assert_eq!(shown(&editor), 2);
}

#[test]
fn a_prompt_opened_around_a_secret_wants_no_dictionary_once_it_no_longer_shows() {
    // As when the window that showed the secret shows another buffer under the prompt.
    let mut editor = searching(Buffer::from_text("t", ""));
    editor.prompts.last_mut().unwrap().around_secret = true;
    assert!(!editor.shows_secret());
    typing(&mut editor, "kanji");
    assert!(editor.take_search_request().is_some());
    assert!(!editor.take_migemo_dictionary_wanted());
    let mut editor = completing(Buffer::from_text("t", ""), &names());
    editor.prompts.last_mut().unwrap().around_secret = true;
    typing(&mut editor, "%kan");
    assert!(!editor.take_migemo_dictionary_wanted());
}

#[test]
fn no_query_keeps_a_table_alive() {
    let mut editor = searching(Buffer::from_text("t", ""));
    let table = dictionary();
    editor.set_migemo_dictionary(table.clone());
    typing(&mut editor, "kanji");
    let request = editor.take_search_request().unwrap();
    editor.rows_found(request.generation, Vec::new(), Some(WalkEnd { cut: false }));
    drop(request);
    let mut completing = completing(Buffer::from_text("t", ""), &names());
    completing.set_migemo_dictionary(table.clone());
    typing(&mut completing, "%kan");
    // The editors hold one each, and their queries none.
    assert_eq!(Arc::strong_count(table.as_ref().unwrap()), 3);
    editor.set_migemo_dictionary(dictionary());
    completing.set_migemo_dictionary(None);
    assert_eq!(Arc::strong_count(table.as_ref().unwrap()), 1);
}
