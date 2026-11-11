//! query-replace (ADR 0113): each slice `!` replaces on the UI thread, over a large file with
//! many matches, and a regexp over a long line on the worker. Run with `-- replace`.
//!
//! Must not depend on: other workspace crates or crate internals; it uses only the public
//! API.

use std::time::{Duration, Instant};

use oso_core::buffer::Buffer;
use oso_core::editor::{Editor, ReplaceAnswer};

use super::{MB, file_text, single_line};

/// p50, p99 and the most of `times`.
fn spread(mut times: Vec<Duration>) -> (Duration, Duration, Duration) {
    times.sort_unstable();
    let at = |share: f64| {
        let index = ((times.len() as f64 - 1.0) * share).round() as usize;
        times.get(index).copied().unwrap_or_default()
    };
    (at(0.5), at(0.99), times.last().copied().unwrap_or_default())
}

pub(crate) fn replace_costs() {
    println!("== query-replace's ! (ADR 0113)");
    // 100 MB of source-like text, with a word on a line of its own every 25 lines.
    let source = file_text(100 * MB);
    let mut text = String::with_capacity(source.len() + 1_000_000);
    let mut placed = 0;
    for (index, line) in source.split_inclusive('\n').enumerate() {
        text.push_str(line);
        if index % 25 == 0 && placed < 100_000 {
            text.push_str("needle\n");
            placed += 1;
        }
    }
    let mut editor = Editor::with_buffer(Buffer::from_text("file", &text));
    let started = Instant::now();
    editor.begin_undo_group();
    editor
        .replace_begin("needle", "thread", false)
        .expect("the buffer takes edits");
    editor.end_undo_group();
    let begun = started.elapsed();
    editor.replace_answer(ReplaceAnswer::All);
    let mut slices = Vec::new();
    while editor.replace_works() {
        let started = Instant::now();
        editor.replace_continue();
        slices.push(started.elapsed());
    }
    let count = slices.len();
    let (p50, p99, most) = spread(slices);
    println!(
        "100 MB, {placed} matches: begin {begun:>9.2?}; {count} slices p50 {p50:>9.2?} p99 {p99:>9.2?} most {most:>9.2?}"
    );
    let started = Instant::now();
    editor.undo().expect("the session undoes");
    println!("undo of the whole session  {:>9.2?}", started.elapsed());

    // A regexp over a 10 MB line, one match at its end: the key's budget, then the worker.
    let line = format!("{}key=42", single_line(10 * MB));
    let mut editor = Editor::with_buffer(Buffer::from_text("line", &line));
    let started = Instant::now();
    editor
        .replace_begin(r"key=(\d+)", "$1", true)
        .expect("the buffer takes edits");
    let on_key = started.elapsed();
    let started = Instant::now();
    while let Some(job) = editor.take_isearch_job() {
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        for report in reports {
            editor.isearch_reported(report);
        }
    }
    println!(
        "10 MB line, a regexp: key {on_key:>9.2?}; worker find and count {:>9.2?}",
        started.elapsed()
    );
}
