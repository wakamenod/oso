//! Typing at many cursors, bare, with marks and over regions, and undoing it, on a large
//! file and on long lines (ADR 0091). Run with `-- cursors`.
//!
//! Must not depend on: crate internals; it uses only the public API, and the text and
//! timing helpers of `text_perf`.

use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, Replacement};
use oso_core::editor::{Editor, Toward};
use oso_core::motion;
use oso_core::selection::{MarkActive, Selection, SelectionSet};

use super::{MB, editor_with_cursors, file_text, percentiles, single_line, typing_costs};

/// About `bytes` of one line of ASCII, as minified JSON is.
fn json_line(bytes: usize) -> String {
    let piece = r#"{"id":12345,"name":"value","tags":["a","b"],"ok":true},"#;
    piece.repeat(bytes / piece.len() + 1)
}

/// The cursors of `editor`, each with its mark about `width` bytes before it, and the
/// region shown if `shown`, as `C->` leaves them (ADR 0091).
fn mark_every_cursor(editor: &mut Editor, width: usize, shown: bool) {
    let (window, buffer) = editor.current_mut();
    let display = buffer.display();
    let parts: Vec<_> = window
        .selections
        .iter()
        .map(|selection| Selection {
            cursor: selection.cursor,
            mark: Some(motion::cluster_start(
                &display,
                selection.cursor.saturating_sub(width),
            )),
            goal: None,
        })
        .collect();
    let mut set = SelectionSet::from_parts(parts, 0).expect("there is a cursor");
    set.set_mark_active(if shown {
        MarkActive::Active
    } else {
        MarkActive::Inactive
    });
    window.selections = set;
}

/// One command that types "x" over each cursor's region if it shows, and at the cursor
/// otherwise.
fn type_over_regions(editor: &mut Editor) {
    let id = editor.window().buffer();
    let selections = &editor.window().selections;
    let shown = selections.mark_active().is_active();
    let edits: Vec<_> = selections
        .iter()
        .map(|selection| Replacement {
            range: if shown {
                selection.range()
            } else {
                selection.cursor..selection.cursor
            },
            text: "x",
        })
        .collect();
    editor.begin_undo_group();
    editor
        .apply_edits(id, &edits)
        .expect("the buffer is editable");
    editor.end_undo_group();
}

/// p50 and p99 of typing at `cursors` cursors that each have a mark, over shown regions of
/// 3 bytes when `shown`, and of undoing it.
fn marked_typing_costs(label: &str, text: &str, cursors: usize, shown: bool) {
    let commands = (20_000 / cursors).clamp(200, 10_000);
    let mut editor = editor_with_cursors(text, cursors);
    let mut typed = Vec::with_capacity(commands);
    for _ in 0..commands {
        mark_every_cursor(&mut editor, 3, shown);
        let start = Instant::now();
        type_over_regions(&mut editor);
        typed.push(start.elapsed());
    }
    let mut undone = Vec::with_capacity(commands);
    for _ in 0..commands {
        let start = Instant::now();
        editor.undo().expect("a command to undo");
        undone.push(start.elapsed());
        // The app frees what an undo lets go off the UI thread (ADR 0119).
        drop(editor.take_crossing_freed());
    }
    let (typed_50, typed_99) = percentiles(&mut typed);
    let (undone_50, undone_99) = percentiles(&mut undone);
    let what = if shown { "regions" } else { "marks" };
    println!(
        "{label:<12} {cursors:>5} {what:<7}  type p50 {typed_50:>9.2?} p99 {typed_99:>9.2?}   undo p50 {undone_50:>9.2?} p99 {undone_99:>9.2?}"
    );
}

/// Typing at many cursors, bare, with marks and over regions (ADR 0091).
pub(crate) fn cursor_costs() {
    search_costs();
    println!();
    println!("== Typing at many cursors, and undo (ADR 0091)");
    let text = file_text(100 * MB);
    let line = single_line(10 * MB);
    let json = json_line(10 * MB);
    for (label, text) in [
        ("100 MB file", &text),
        ("10 MB line", &line),
        ("10 MB JSON", &json),
    ] {
        for cursors in [1, 100, 1000] {
            typing_costs(label, text, cursors);
            marked_typing_costs(label, text, cursors, false);
            marked_typing_costs(label, text, cursors, true);
        }
    }
}

/// What a key that adds cursors takes on the UI thread, and the worker after it, as
/// `(key, worker)`. `key` runs it on an editor showing `text` with one cursor at `at`.
fn search_time(text: &str, at: usize, key: impl Fn(&mut Editor)) -> (Duration, Duration) {
    let mut editor = Editor::with_buffer(Buffer::from_text("text", text));
    editor.current_mut().0.selections = SelectionSet::new(at);
    let start = Instant::now();
    key(&mut editor);
    let on_key = start.elapsed();
    let start = Instant::now();
    if let Some(job) = editor.take_isearch_job() {
        job.run(|report| {
            let _ = editor.isearch_reported(report);
        });
    }
    (on_key, start.elapsed())
}

/// Keys that add cursors where every match but the last is checked and rejected, and on
/// every occurrence of a word in source (ADR 0091).
fn search_costs() {
    println!("== Searches for cursors: on the key, and on the worker");
    for size in [64 * 1024, 256 * 1024, MB] {
        let back = format!("{}i", "fix ".repeat(size / 4));
        let (key, worker) = search_time(&back, back.len(), |editor| {
            let _ = editor.mark_like_this(Toward::Previous, 1);
        });
        println!(
            "{:>5} KB  C-< past rejected words  key {key:>9.2?}  worker {worker:>9.2?}",
            size / 1024
        );
        let forward = format!("i{}", " fix".repeat(size / 4));
        let (key, worker) = search_time(&forward, 0, |editor| {
            let _ = editor.mark_like_this(Toward::Next, 1);
        });
        println!(
            "{:>5} KB  C-> past rejected words  key {key:>9.2?}  worker {worker:>9.2?}",
            size / 1024
        );
        let run = format!("a {}", "a".repeat(size));
        let (key, worker) = search_time(&run, 0, |editor| {
            let _ = editor.mark_like_this(Toward::Next, 1);
        });
        println!(
            "{:>5} KB  C-> in a run of letters  key {key:>9.2?}  worker {worker:>9.2?}",
            size / 1024
        );
        let source = file_text(size);
        let main = source.find("main").unwrap_or(0);
        let (key, worker) = search_time(&source, main, |editor| {
            let _ = editor.mark_all_like_this();
        });
        println!(
            "{:>5} KB  C-c C-< on source        key {key:>9.2?}  worker {worker:>9.2?}",
            size / 1024
        );
    }
}
