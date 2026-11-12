//! Tests of migemo on the editor (ADR 0116): a romaji string's kana in isearch, its line,
//! `M-m`, the setting, `M-r` and `M-c` with it, the history, cursors from it, the cache
//! around a secret, a jump's string, and a long line's limit.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::test_buffers::secret_buffer;
use super::{Editor, Status};
use crate::buffer::Buffer;
use crate::decode::InvalidBytes;
use crate::editor::Toward;
use crate::jump::Kind;
use crate::rope::Rope;
use crate::search::Needle;
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
fn del_and_a_repeat_compile_nothing_and_a_secret_keeps_no_needle() {
    let mut editor = editor("か ka");
    search(&mut editor, "k");
    editor.isearch_add("a");
    assert_eq!(editor.migemo.cached(), 2);
    editor.isearch_delete();
    editor.isearch_exit();
    search(&mut editor, "ka");
    assert_eq!(editor.migemo.cached(), 2);

    let mut buffer = secret_buffer();
    buffer.apply_load(crate::load::decode_all(b"TOKEN=kana", Default::default()));
    let mut editor = Editor::with_buffer(buffer);
    search(&mut editor, "kana");
    assert!(migemo(&editor));
    assert_eq!(editor.migemo.cached(), 0);
    assert!(!format!("{:?}", editor.migemo).contains("kana"));
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
fn a_long_line_reads_no_further_than_the_limit() {
    let needle = Needle::migemo("ka", false).unwrap();
    // A match lies far into a line of 8 MiB.
    let mut text = "x".repeat(8 * 1024 * 1024);
    text.push_str("か\n");
    let rope = Rope::from(text.as_str());
    let started = std::time::Instant::now();
    let mut out = Vec::new();
    let whole = 0..rope.len();
    let spans = std::slice::from_ref(&whole);
    needle.matches_in(
        (&rope, &InvalidBytes::new()),
        spans,
        (16, 1024 * 1024),
        &mut out,
    );
    assert!(out.is_empty());
    // A span within the limit is read.
    let end = rope.len() - 64..rope.len();
    let spans = std::slice::from_ref(&end);
    needle.matches_in(
        (&rope, &InvalidBytes::new()),
        spans,
        (16, 1024 * 1024),
        &mut out,
    );
    assert_eq!(out.len(), 1);
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
}

/// What a migemo key costs on the UI thread, its highlights with it, for ADR 0116's notes:
/// run with `cargo test -p oso-core --release --lib migemo_key_costs -- --ignored --nocapture`.
#[test]
#[ignore = "a measure, in a release build"]
#[expect(clippy::disallowed_macros, reason = "the measure prints its timings")]
fn migemo_key_costs() {
    use std::time::Instant;
    let line = "the quick brown fox jumps over the lazy dog 0123456789\n";
    let lines = line.repeat(20 * 1024 * 1024 / line.len());
    let japanese = format!(
        "{}\n",
        "日本語のテキストとかなカナ ".repeat(20 * 1024 * 1024 / 40)
    );
    let long_romaji = "kanjinohenkan".repeat(256 / 13);
    let all_n = "n".repeat(256);
    let cases: &[(&str, &str, &str)] = &[
        ("lines", &lines, "zzq"),
        ("lines", &lines, "k"),
        ("lines", &lines, "kanji"),
        ("lines", &lines, &long_romaji),
        ("lines", &lines, &all_n),
        ("japanese", &japanese, "kanji"),
        ("japanese", &japanese, "zuq"),
    ];
    for string in ["k", "kanji", &long_romaji, &all_n] {
        let mut times = Vec::new();
        for _ in 0..30 {
            let started = Instant::now();
            let pattern = crate::migemo::pattern(string).unwrap_or_default();
            let built = started.elapsed().as_secs_f64() * 1000.0;
            let _ = Needle::migemo(string, false);
            let both = started.elapsed().as_secs_f64() * 1000.0;
            times.push((both, built, pattern.len()));
        }
        times.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (both, built, bytes) = times[29];
        println!(
            "needle len={:4} pattern={bytes:6} B built={built:.3} max={both:.3} ms",
            string.len()
        );
    }
    for &(what, text, string) in cases {
        let mut times = Vec::new();
        for _ in 0..30 {
            let mut editor = editor(text);
            editor.set_view_size(ViewSize::new(40, 120));
            editor.isearch_begin(false, false);
            let started = Instant::now();
            editor.isearch_add(string);
            let mut cells = super::MatchCells::default();
            editor.isearch_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        let name: String = string.chars().take(20).collect();
        let (p50, max) = (times[15], times[29]);
        println!(
            "{what:8} {name:20} len={:4} p50={p50:.3} max={max:.3} ms",
            string.len()
        );
    }
}
