//! Tests of migemo on the editor (ADR 0116): a romaji string's kana in isearch, its line,
//! `M-m`, the setting, `M-r` and `M-c` with it, the history, cursors from it, the cache
//! and `Debug`, a secret kill yanked into the string, and a jump's string, also around a
//! secret.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use std::sync::atomic::Ordering;

use super::test_buffers::secret_buffer;
use super::{Editor, Status};
use crate::buffer::Buffer;
use crate::editor::Toward;
use crate::jump::Kind;
use crate::search::{Report, SearchReport};
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

fn search(editor: &mut Editor, text: &str) {
    editor.isearch_begin(false, false);
    editor.isearch_add(text);
    drain(editor);
}

fn migemo(editor: &Editor) -> bool {
    editor.isearch().is_some_and(super::Isearch::migemo)
}

#[test]
fn romaji_finds_its_kana_in_every_form_and_the_line_says_migemo() {
    let mut editor = editor("が ka か カ ｶ ｋａ KA");
    search(&mut editor, "ka");
    assert!(migemo(&editor));
    assert_eq!(line(&editor), "[migemo] I-search: ka [1/6]");
    // `が` is not `か`.
    assert_eq!(cursor(&editor), "が ka".len());
    editor.isearch_repeat(false);
    assert_eq!(cursor(&editor), "が ka か".len());
    editor.isearch_exit();
    // The string enters the history as typed.
    assert_eq!(editor.histories.get("isearch", 0), Some("ka"));
}

#[test]
fn any_other_character_searches_literally() {
    for (string, count) in [("Ka", "[1/1]"), ("struct", "[1/1]"), ("ka ", "[1/3]")] {
        let mut editor = editor("ka Ka か struct すとらct ka ");
        search(&mut editor, string);
        assert!(!migemo(&editor), "{string:?}");
        assert_eq!(line(&editor), format!("I-search: {string} {count}"));
    }
}

#[test]
fn m_m_turns_it_off_for_this_search_and_del_takes_that_back() {
    let mut editor = editor("か ka");
    search(&mut editor, "ka");
    assert_eq!(cursor(&editor), "か".len());
    editor.isearch_toggle_migemo();
    drain(&mut editor);
    assert!(!migemo(&editor));
    assert_eq!(line(&editor), "I-search: ka [1/1]");
    assert_eq!(cursor(&editor), "か ka".len());
    assert!(editor.isearch_delete());
    drain(&mut editor);
    assert!(migemo(&editor));
    // The next search starts as the setting says.
    editor.isearch_exit();
    search(&mut editor, "ka");
    assert!(migemo(&editor));
}

#[test]
fn the_setting_off_starts_searches_literal_and_m_m_turns_migemo_on() {
    let mut editor = editor("か ka");
    editor.set_migemo(false);
    search(&mut editor, "ka");
    assert!(!migemo(&editor));
    editor.isearch_toggle_migemo();
    drain(&mut editor);
    assert!(migemo(&editor));
    // It searches again from the match, which still matches.
    assert_eq!(line(&editor), "[migemo] I-search: ka [2/2]");
}

#[test]
fn a_regexp_search_is_never_migemo_s_and_m_m_there_shows_after_m_r() {
    let mut editor = editor("か ka");
    editor.isearch_begin(false, true);
    editor.isearch_add("ka");
    drain(&mut editor);
    assert!(!migemo(&editor));
    assert_eq!(line(&editor), "Regexp I-search: ka [1/1]");
    editor.isearch_toggle_regexp();
    drain(&mut editor);
    assert!(migemo(&editor));
    // `M-m` in a regexp's search turns the switch, which shows once it is literal.
    editor.isearch_toggle_regexp();
    editor.isearch_toggle_migemo();
    drain(&mut editor);
    assert!(!migemo(&editor));
    editor.isearch_toggle_regexp();
    drain(&mut editor);
    assert!(!migemo(&editor));
    assert_eq!(line(&editor), "I-search: ka [1/1]");
}

#[test]
fn m_c_makes_the_letters_case_sensitive_and_kana_have_no_case() {
    let mut editor = editor("KA ka か");
    search(&mut editor, "ka");
    assert_eq!(line(&editor), "[migemo] I-search: ka [1/3]");
    editor.isearch_toggle_case();
    drain(&mut editor);
    assert_eq!(line(&editor), "[migemo] Case-sensitive I-search: ka [1/2]");
}

#[test]
fn backward_finds_the_kana_before_the_cursor() {
    let mut editor = editor("か ka カ");
    let len = editor.current().1.text().len();
    editor.window_mut(0).0.selections = SelectionSet::new(len);
    editor.isearch_begin(true, false);
    editor.isearch_add("ka");
    drain(&mut editor);
    assert_eq!(cursor(&editor), "か ka ".len());
    assert!(migemo(&editor));
}

#[test]
fn cursors_from_the_search_go_on_migemo_s_matches() {
    let mut editor = editor("か x カ x ka");
    search(&mut editor, "ka");
    editor.isearch_mark(Some(Toward::Next)).unwrap();
    let starts: Vec<usize> = editor
        .window()
        .selections
        .iter()
        .map(|selection| selection.range().start)
        .collect();
    assert_eq!(starts, [0, "か x ".len()]);
}

#[test]
fn del_and_a_repeat_compile_nothing() {
    let mut editor = editor("か ka");
    search(&mut editor, "k");
    editor.isearch_add("a");
    assert_eq!(editor.migemo.cached(), 2);
    editor.isearch_delete();
    editor.isearch_exit();
    search(&mut editor, "ka");
    assert_eq!(editor.migemo.cached(), 2);
}

/// An editor that shows a buffer that may hold secrets, holding `kana`.
fn secret_editor() -> Editor {
    let mut buffer = secret_buffer();
    buffer.apply_load(crate::load::decode_all(b"TOKEN=kana", Default::default()));
    Editor::with_buffer(buffer)
}

#[test]
fn debug_prints_no_cached_string_and_a_secret_keeps_no_needle() {
    let mut editor = editor("かな kana");
    search(&mut editor, "kanaka");
    editor.isearch_exit();
    assert!(editor.migemo.cached() > 0);
    assert!(!format!("{:?}", editor.migemo).contains("kanaka"));

    let mut editor = secret_editor();
    search(&mut editor, "kana");
    assert!(migemo(&editor));
    assert_eq!(editor.migemo.cached(), 0);
}

#[test]
fn a_secret_kill_of_romaji_yanked_into_the_string_keeps_no_needle() {
    let mut editor = secret_editor();
    let direction = crate::kill::Direction::Forward;
    editor
        .kill(std::iter::once(6..10), direction, true)
        .unwrap();
    let notes = editor.add_buffer(Buffer::from_text("notes.txt", "かな kana"));
    editor.show(notes);
    assert!(!editor.shows_secret());
    editor.isearch_begin(false, false);
    editor.isearch_yank_kill();
    assert_eq!(editor.isearch().unwrap().string(), "kana");
    assert!(migemo(&editor), "the string is still searched by migemo");
    assert_eq!(editor.migemo.cached(), 0);
}

#[test]
fn a_jump_s_string_matches_by_migemo_as_the_setting_says() {
    let mut editor = editor("か x ka x");
    let targets = |editor: &mut Editor| {
        assert!(editor.jump_begin(Kind::Char, false));
        editor.jump_add("ka");
        editor.jump_show_labels();
        let count = editor.jump().map_or(0, |jump| jump.count());
        editor.jump_end();
        count
    };
    assert_eq!(targets(&mut editor), 2);
    editor.set_migemo(false);
    assert_eq!(targets(&mut editor), 1);
}

#[test]
fn a_jump_around_a_secret_keeps_no_needle() {
    let mut editor = secret_editor();
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add("ka");
    assert_eq!(editor.migemo.cached(), 0);
    // The jump stays around the secret after the window shows another buffer.
    let notes = editor.add_buffer(Buffer::from_text("notes.txt", "か ka"));
    editor.show(notes);
    assert!(!editor.shows_secret());
    editor.jump_add("n");
    assert_eq!(editor.migemo.cached(), 0);
    editor.jump_end();
    // A jump that began elsewhere keeps its needle.
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add("ka");
    assert_eq!(editor.migemo.cached(), 1);
}

#[test]
fn a_line_too_long_for_a_regexp_takes_the_search_on_literally() {
    let mut editor = editor("か\nxxxxxxx ka\nka");
    search(&mut editor, "ka");
    assert_eq!(cursor(&editor), "か".len());
    editor.isearch_longest_line(5);
    editor.isearch_repeat(false);
    drain(&mut editor);
    // The literal string from where it stopped, with no `[migemo]` and no stop.
    assert!(!migemo(&editor));
    assert_eq!(cursor(&editor), "か\nxxxxxxx ka".len());
    assert_eq!(line(&editor), "I-search: ka [1/2]");
    // `DEL` goes back to migemo's step, as after `M-m`.
    assert!(editor.isearch_delete());
    assert!(migemo(&editor));
    assert_eq!(cursor(&editor), "か".len());

    // The worker's count meeting one takes it on literally too.
    let mut editor = self::editor("か ka");
    search(&mut editor, "ka");
    let live = editor.search_live.load(Ordering::Relaxed);
    let report = SearchReport(Report::LineTooLong {
        generation: live,
        at: 0,
    });
    assert!(editor.isearch_reported(report));
    drain(&mut editor);
    assert!(!migemo(&editor));
    assert_eq!(line(&editor), "I-search: ka [1/1]");
}
