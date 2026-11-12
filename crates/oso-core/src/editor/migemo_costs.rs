//! Measures of migemo on the editor (ADR 0116), ignored by default, for the ADR's notes: what
//! a key costs on the UI thread with its highlights, also as a string is typed and taken back,
//! what a line too long for a regexp does, and a jump around a secret, which caches nothing.
//! Run in a release build on an idle machine, with step 1's `key_costs`:
//! `cargo test -p oso-core --release --lib key_costs -- --ignored --nocapture --test-threads 1`.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

#![expect(clippy::disallowed_macros, reason = "the measures print their timings")]

use std::time::Instant;

use super::Editor;
use super::migemo::Cache;
use crate::buffer::Buffer;
use crate::frame::Way;
use crate::jump::Kind;
use crate::search::Needle;
use crate::selection::SelectionSet;
use crate::window::ViewSize;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_view_size(ViewSize::new(40, 120));
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

/// Milliseconds since `started`.
fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// The median and the largest of `times`, sorted.
fn p50_max(times: &mut [f64]) -> (f64, f64) {
    times.sort_by(f64::total_cmp);
    (times[times.len() / 2], times[times.len() - 1])
}

/// What a frame's highlights cost on top of a key.
fn highlights(editor: &Editor) {
    let mut cells = super::MatchCells::default();
    editor.isearch_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
}

/// 20 MB of short English lines, and of short Japanese lines.
fn texts() -> [(&'static str, String); 2] {
    let line = "the quick brown fox jumps over the lazy dog 0123456789\n";
    let japanese = "日本語のテキストとかなカナの行です。\n";
    [
        ("lines", line.repeat(20 * 1024 * 1024 / line.len())),
        (
            "japanese",
            japanese.repeat(20 * 1024 * 1024 / japanese.len()),
        ),
    ]
}

/// The strings measured: a literal one, short romaji, and romaji of 256 characters, the last
/// four the largest patterns.
fn strings() -> Vec<(&'static str, String)> {
    let long = |unit: &str| unit.repeat(256 / unit.len());
    vec![
        ("zzq", "zzq".to_owned()),
        ("k", "k".to_owned()),
        ("kanji", "kanji".to_owned()),
        ("kanjinohenkan*", long("kanjinohenkan")),
        ("n*256", long("n")),
        ("a*256", long("a")),
        ("aiueo*", long("aiueo")),
        ("ga*128", long("ga")),
        ("vu*128", long("vu")),
    ]
}

/// What a migemo key costs on the UI thread, its highlights with it.
#[test]
#[ignore = "a measure, in a release build"]
fn migemo_key_costs() {
    for (name, string) in strings() {
        // The pattern alone, and the needle, which builds it and compiles it.
        let (mut patterns, mut needles) = (Vec::new(), Vec::new());
        let mut bytes = 0;
        for _ in 0..30 {
            let started = Instant::now();
            bytes = crate::migemo::pattern(&string).map_or(0, |pattern| pattern.len());
            patterns.push(ms(started));
            let started = Instant::now();
            std::hint::black_box(Needle::migemo(&string, false));
            needles.push(ms(started));
        }
        let (pattern_p50, pattern_max) = p50_max(&mut patterns);
        let (p50, max) = p50_max(&mut needles);
        println!(
            "needle  {name:15} pattern={bytes:6} B build p50={pattern_p50:.3} \
             max={pattern_max:.3} needle p50={p50:.3} max={max:.3} ms"
        );
    }
    for (what, text) in texts() {
        for (name, string) in strings() {
            for backward in [false, true] {
                let mut times = Vec::new();
                for _ in 0..30 {
                    let mut editor = editor(&text);
                    if backward {
                        let len = editor.current().1.text().len();
                        editor.window_mut(0).0.selections = SelectionSet::new(len);
                    }
                    editor.isearch_begin(backward, false);
                    let started = Instant::now();
                    editor.isearch_add(&string);
                    highlights(&editor);
                    times.push(ms(started));
                }
                let (p50, max) = p50_max(&mut times);
                println!("{what:8} {name:15} back={backward:5} p50={p50:.3} max={max:.3} ms");
            }
        }
    }
}

/// What each key costs as 256 characters of romaji are typed one at a time and then taken
/// back with `DEL`, forward and backward: the first time each string's needle is built, and
/// `DEL` takes the cached one.
#[test]
#[ignore = "a measure, in a release build"]
fn migemo_typing_costs() {
    let [(_, text), _] = texts();
    for unit in ["kanjinohenkan", "a", "aiueo", "ga"] {
        let string = unit.repeat(256 / unit.len());
        for backward in [false, true] {
            let (mut typed, mut deleted) = (Vec::new(), Vec::new());
            for _ in 0..3 {
                let mut editor = editor(&text);
                if backward {
                    let len = editor.current().1.text().len();
                    editor.window_mut(0).0.selections = SelectionSet::new(len);
                }
                editor.isearch_begin(backward, false);
                for c in string.chars() {
                    let started = Instant::now();
                    editor.isearch_add(c.encode_utf8(&mut [0; 4]));
                    highlights(&editor);
                    typed.push(ms(started));
                    drain(&mut editor);
                }
                while {
                    let started = Instant::now();
                    let more = editor.isearch_delete();
                    highlights(&editor);
                    deleted.push(ms(started));
                    more
                } {
                    drain(&mut editor);
                }
            }
            let (p50, max) = p50_max(&mut typed);
            let (del_p50, del_max) = p50_max(&mut deleted);
            println!(
                "typing {unit:13} back={backward:5} key p50={p50:.3} max={max:.3} \
                 DEL p50={del_p50:.3} max={del_max:.3} ms"
            );
        }
    }
}

/// A word that is romaji, `name`, in one line of 100 MB, past which a regexp is not matched:
/// literal, as before migemo, and migemo's, the key and the worker after it.
#[test]
#[ignore = "a measure, in a release build"]
fn migemo_on_a_line_too_long_for_a_regexp() {
    let mut text = "x".repeat(100 * 1024 * 1024);
    text.push_str("name\n");
    for migemo in [false, true] {
        let mut editor = editor(&text);
        editor.set_migemo(migemo);
        editor.isearch_begin(false, false);
        let started = Instant::now();
        editor.isearch_add("name");
        let key = ms(started);
        drain(&mut editor);
        let all = ms(started);
        let state = editor.isearch().unwrap();
        println!(
            "100 MB line migemo={migemo:5} key={key:.3} ms with worker={all:.1} ms \
             failing={} count={:?}",
            state.failing(),
            state.count()
        );
    }
}

/// A jump's editor over `text`, 120 rows by 400 columns, in `windows` of 1, or 4 of 60 by 200,
/// a buffer that may hold secrets if `secret`.
fn jump_editor(text: &str, windows: usize, secret: bool) -> Editor {
    let mut editor = if secret {
        let mut buffer = super::test_buffers::secret_buffer();
        let load = crate::load::decode_all(text.as_bytes(), Default::default());
        buffer.apply_load(load);
        Editor::with_buffer(buffer)
    } else {
        Editor::with_buffer(Buffer::from_text("notes.txt", text))
    };
    editor.set_view_size(ViewSize::new(120, 400));
    if windows == 4 {
        let top = editor.window().id();
        let bottom = editor.split_window(Way::Down, false).unwrap();
        editor.split_window(Way::Across, false).unwrap();
        editor.select_window(bottom);
        editor.split_window(Way::Across, false).unwrap();
        editor.select_window(top);
    }
    editor
}

/// What a jump's key costs, in 1 window and in 4, with its needle from the cache and, around a
/// secret, built each time: short strings, and one of the jump's 64 characters.
#[test]
#[ignore = "a measure, in a release build"]
fn migemo_jump_costs() {
    let long = "a".repeat(crate::jump::MAX_STRING);
    for (what, text) in texts() {
        for windows in [1, 4] {
            for string in ["k", "ka", "no", long.as_str()] {
                for secret in [false, true] {
                    let mut times = Vec::new();
                    for _ in 0..20 {
                        let mut editor = jump_editor(&text, windows, secret);
                        // Once, which fills the cache unless around a secret.
                        assert!(editor.jump_begin(Kind::Char, false));
                        editor.jump_add(string);
                        editor.jump_end();
                        assert!(editor.jump_begin(Kind::Char, false));
                        let started = Instant::now();
                        editor.jump_add(string);
                        times.push(ms(started));
                    }
                    let (p50, max) = p50_max(&mut times);
                    let name = if string.len() > 8 { "a*64" } else { string };
                    println!(
                        "jump {what:8} windows={windows} {name:4} secret={secret:5} \
                         p50={p50:.3} max={max:.3} ms"
                    );
                }
            }
        }
    }
}

/// Fills the cache with `count` of the largest needles and holds them, for a run under
/// `/usr/bin/time -l` that compares the peak memory of 32 with that of 1.
fn fill_cache(count: usize) {
    let mut editor = editor("");
    for at in 0..count {
        let string = "a".repeat(256 - at);
        assert!(
            editor
                .migemo
                .string_needle(&string, false, (true, Cache::Keep))
                .is_some()
        );
    }
    assert_eq!(editor.migemo.cached(), count);
    std::hint::black_box(&editor);
}

#[test]
#[ignore = "a measure of memory, alone under /usr/bin/time -l"]
fn migemo_cache_memory_1() {
    fill_cache(1);
}

#[test]
#[ignore = "a measure of memory, alone under /usr/bin/time -l"]
fn migemo_cache_memory_32() {
    fill_cache(32);
}
