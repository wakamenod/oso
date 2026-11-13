//! Tests of migemo's dictionary on the editor (ADR 0116): when a search wants one, the
//! search on screen and a jump reading its string looking again when it comes, the cache
//! emptied, a dictionary taken away, and words that would make too large a pattern dropped
//! for the kana. Every dictionary here is synthetic.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use std::sync::Arc;

use super::Editor;
use crate::buffer::Buffer;
use crate::jump::Kind;
use crate::migemo::Dictionary;
use crate::window::ViewSize;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_view_size(ViewSize::new(10, 80));
    editor
}

fn dictionary(text: &str) -> Option<Arc<Dictionary>> {
    Some(Arc::new(Dictionary::new(text.to_owned())))
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

fn cursor(editor: &Editor) -> usize {
    editor.window().selections.primary().cursor
}

fn failing(editor: &Editor) -> bool {
    editor.isearch().is_some_and(super::Isearch::failing)
}

#[test]
fn a_romaji_search_with_migemo_on_wants_a_dictionary_once() {
    let mut editor = editor("か x");
    assert!(!editor.take_migemo_dictionary_wanted());
    editor.isearch_begin(false, false);
    editor.isearch_add("struct");
    assert!(!editor.take_migemo_dictionary_wanted(), "gives no kana");
    editor.isearch_exit();
    editor.isearch_begin(false, false);
    editor.isearch_add("ka");
    assert!(editor.take_migemo_dictionary_wanted());
    assert!(!editor.take_migemo_dictionary_wanted(), "taken");
    editor.isearch_exit();
    // Off, and a regexp's search, want none.
    editor.set_migemo(false);
    editor.isearch_begin(false, false);
    editor.isearch_add("ka");
    editor.isearch_exit();
    editor.set_migemo(true);
    editor.isearch_begin(false, true);
    editor.isearch_add("ka");
    editor.isearch_exit();
    assert!(!editor.take_migemo_dictionary_wanted());
    // With one, none is wanted.
    editor.set_migemo_dictionary(dictionary("か\t蚊\n"));
    editor.isearch_begin(false, false);
    editor.isearch_add("ki");
    assert!(!editor.take_migemo_dictionary_wanted());
}

#[test]
fn a_jump_wants_a_dictionary_too() {
    let mut editor = editor("か x");
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add("ka");
    assert!(editor.take_migemo_dictionary_wanted());
}

#[test]
fn the_search_on_screen_looks_again_when_the_dictionary_comes_with_no_step_of_its_own() {
    let text = "感じ 漢字 x";
    let mut editor = editor(text);
    editor.isearch_begin(false, false);
    for c in ["k", "a", "n", "j", "i"] {
        editor.isearch_add(c);
        drain(&mut editor);
    }
    assert!(failing(&editor));
    editor.set_migemo_dictionary(dictionary("かんじ\t漢字\n"));
    drain(&mut editor);
    assert!(!failing(&editor));
    let end = text.find(" x").unwrap();
    assert_eq!(cursor(&editor), end);
    // `DEL` goes back to `kanj`, not to the search before the dictionary.
    assert!(editor.isearch_delete());
    assert_eq!(editor.isearch().unwrap().string(), "kanj");
}

#[test]
fn a_needle_kept_from_before_the_dictionary_is_built_again() {
    let mut editor = editor("漢字");
    let search = |editor: &mut Editor| {
        editor.isearch_begin(false, false);
        editor.isearch_add("kanji");
        drain(editor);
        let found = !failing(editor);
        editor.isearch_exit();
        editor.window_mut(0).0.selections = crate::selection::SelectionSet::new(0);
        found
    };
    assert!(!search(&mut editor));
    assert!(editor.migemo.cached() > 0);
    editor.set_migemo_dictionary(dictionary("かんじ\t漢字\n"));
    assert_eq!(editor.migemo.cached(), 0);
    assert!(search(&mut editor));
    // Taken away, the kana alone again.
    editor.set_migemo_dictionary(None);
    assert!(!search(&mut editor));
}

#[test]
fn a_jump_reading_its_string_finds_its_matches_again_when_the_dictionary_comes() {
    let mut editor = editor("漢字 かんじ");
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add("kanji");
    editor.set_migemo_dictionary(dictionary("かんじ\t漢字\n"));
    editor.jump_show_labels();
    assert_eq!(editor.jump().map(|jump| jump.count()), Some(2));
}

#[test]
fn words_nested_past_the_limit_are_dropped_for_the_kana() {
    // Each word goes one further than the last, so the trie nests a group a character.
    let words: Vec<String> = (0..40).map(|n| format!("{}y", "x".repeat(n))).collect();
    // With the words, `xxxxy` would match first.
    let text = "xxxxy か";
    let mut editor = editor(text);
    editor.set_migemo_dictionary(dictionary(&format!("か\t{}\n", words.join("\t"))));
    editor.isearch_begin(false, false);
    editor.isearch_add("ka");
    drain(&mut editor);
    assert!(editor.isearch().unwrap().migemo());
    assert_eq!(cursor(&editor), text.len());
}

#[test]
fn a_search_around_a_secret_wants_no_dictionary() {
    let mut buffer = super::test_buffers::secret_buffer();
    buffer.apply_load(crate::load::decode_all(b"TOKEN=kana", Default::default()));
    let mut editor = Editor::with_buffer(buffer);
    editor.isearch_begin(false, false);
    editor.isearch_add("ka");
    editor.isearch_exit();
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add("ka");
    editor.jump_end();
    assert!(!editor.take_migemo_dictionary_wanted());
}
