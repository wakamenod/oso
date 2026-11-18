//! Measures of `consult-line` (ADR 0154), ignored by default, for the ADR's notes: what the UI
//! thread spends opening the prompt, on a key, taking in a batch and on the preview of a
//! frame, and what the host's thread spends searching a large buffer, an ordinary one and
//! crafted ones. Run in a release build on an idle machine:
//! `cargo test -p oso-core --release --lib lines_costs -- --ignored --nocapture --test-threads 1`.
//!
//! Must not depend on: `command` or `keymap`, threads or files; these run on the editor alone.

#![expect(clippy::disallowed_macros, reason = "the measures print their timings")]

use std::time::Instant;

use super::*;
use crate::buffer::Buffer;
use crate::editor::edit_tests::edit;
use crate::window::ViewSize;

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// An editor showing `text`, its cursor halfway, and how long opening the search took.
fn opened(text: &str) -> (Editor, f64) {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
    editor.set_view_size(ViewSize::new(50, 200));
    editor
        .current_mut()
        .0
        .selections
        .move_primary(text.len() / 2);
    let started = Instant::now();
    editor.open_line_search("consult-line").unwrap();
    (editor, ms(started))
}

/// Types `text` and gives how long the UI thread took to start its search.
fn key(editor: &mut Editor, text: &str) -> f64 {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    let started = Instant::now();
    editor.match_completion();
    ms(started)
}

/// Runs the search asked for: its time, its rows and how it ended.
fn run(editor: &mut Editor) -> (f64, Vec<LineRow>, LinesEnd) {
    let request = editor.take_line_search_request().unwrap();
    let mut rows = Vec::new();
    let started = Instant::now();
    let end = request.run(&mut |row| {
        rows.extend(row);
        true
    });
    (ms(started), rows, end)
}

/// Takes `rows` in as the host sends them, the first alone and the rest with the end: how long
/// the two took together.
fn take_in(editor: &mut Editor, mut rows: Vec<LineRow>, end: LinesEnd) -> f64 {
    let generation = editor.project_search.generation;
    let rest = rows.split_off(rows.len().min(1));
    let started = Instant::now();
    editor.lines_found(generation, rows, None);
    let end = match end {
        LinesEnd::Ended(end) => Some(end),
        LinesEnd::Stopped => None,
    };
    editor.lines_found(generation, rest, end);
    ms(started)
}

/// Opens the search over `text`, then types each of `inputs` in turn, and prints what each
/// step cost.
fn report(name: &str, text: &str, inputs: &[&str]) {
    let (mut editor, open) = opened(text);
    let (all, rows, end) = run(&mut editor);
    let taken = take_in(&mut editor, rows, end);
    let mut line = format!(
        "{name}: {} MiB, open {open:.3} ms, every line {all:.1} ms (taken in {taken:.2} ms)",
        text.len() >> 20
    );
    for input in inputs {
        let typed = key(&mut editor, input);
        let narrowed = editor
            .project_search
            .request
            .as_ref()
            .is_some_and(|request| matches!(request, super::super::Request::Lines(request) if request.narrow.is_some()));
        let (searched, rows, end) = run(&mut editor);
        let count = rows.len();
        let taken = take_in(&mut editor, rows, end);
        let how = if narrowed { "narrowed" } else { "whole" };
        line.push_str(&format!(
            "; `{input}` key {typed:.3} ms, {how} search {searched:.1} ms for {count} rows {end:?}, taken in {taken:.2} ms"
        ));
    }
    let started = Instant::now();
    for _ in 0..100 {
        std::hint::black_box(editor.preview());
    }
    line.push_str(&format!("; preview {:.4} ms", ms(started) / 100.0));
    println!("{line}");
}

#[test]
#[ignore = "a measure, run by hand in release"]
fn lines_costs() {
    // An ordinary source file: 5000 lines.
    let code: String = (0..5000)
        .map(|n| format!("    let value_{n} = compute(input, {n}); // a comment\n"))
        .collect();
    report("5000 lines", &code, &["value_49"]);
    // A large log: a million lines of 100 bytes.
    let log: String = (0..1_000_000)
        .map(|n| format!("{n:>10} INFO request served path=/api/items/{n} status=200 time=3ms\n"))
        .collect();
    report("1M lines", &log, &["items/99999", "9 "]);
    report("1M lines, the same input at once", &log, &["items/999999 "]);
    report("1M lines", &log, &["INFO"]);
    report("1M lines", &log, &["e"]);
    report("1M lines", &log, &["zzzz"]);
    report("1M lines", &log, &["s.*t.*9$"]);
    report("1M lines", &log, &["kanji"]);
    report("1M lines", &log, &["!INFO"]);
    // At the size that narrows: 8 MiB of 64-byte lines, from 5000 and 9500 rows found, narrowed
    // and at once.
    let eight: String = (0..NARROW_PAST / 64)
        .map(|n| {
            let word = match n % 26 {
                0 => "hit",
                1 if n % 52 == 1 => "hot",
                _ if n % 29 == 2 => "hut",
                _ => "---",
            };
            format!("{n:>59} {word}\n")
        })
        .collect();
    report("8 MiB, 5000 rows", &eight, &["hi", "t"]);
    report("8 MiB, 5000 rows at once", &eight, &["hit"]);
    report("8 MiB, 9500 rows", &eight, &["h !o", " !u"]);
    report("8 MiB, 9500 rows at once", &eight, &["h !o !u"]);
    // Crafted: one line of 64 MiB, a narrowed search over 200 lines of 1 MiB, and a million
    // empty lines.
    let long = "ab".repeat(32 << 20);
    report("one 64 MiB line", &long, &["ba"]);
    let wide: String = (0..200)
        .map(|_| format!("{}\n", "a".repeat(1 << 20)))
        .collect();
    report("200 lines of 1 MiB", &wide, &["a", "a"]);
    let empty = "\n".repeat(1_000_000);
    report("1M empty lines", &empty, &["a"]);
}

/// How long a search takes to stop once a newer one moves the generation, over one 64 MiB line
/// and over 200 lines of 1 MiB narrowed.
#[test]
#[ignore = "a measure, run by hand in release"]
fn lines_stop_costs() {
    let stop = |text: &str, inputs: &[&str]| {
        let (mut editor, _) = opened(text);
        let _ = run(&mut editor);
        for input in &inputs[..inputs.len() - 1] {
            key(&mut editor, input);
            let (_, rows, end) = run(&mut editor);
            take_in(&mut editor, rows, end);
        }
        key(&mut editor, inputs[inputs.len() - 1]);
        let request = editor.take_line_search_request().unwrap();
        let live = std::sync::Arc::clone(&request.live);
        let started = Instant::now();
        let end = request.run(&mut |_| {
            live.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        });
        (ms(started), end)
    };
    let long = "ab".repeat(32 << 20);
    println!("one 64 MiB line: {:?}", stop(&long, &["zz"]));
    let wide: String = (0..200)
        .map(|_| format!("{}\n", "a".repeat(1 << 20)))
        .collect();
    println!(
        "200 lines of 1 MiB, narrowed: {:?}",
        stop(&wide, &["a", "a"])
    );
}
