//! Undoing large steps (ADR 0119): a large `!` with what else follows a crossing turned on, a
//! soft wrap, a second window and highlights, a session answered match by match, typing at
//! 1000 cursors joined into one step, a large kill, many runs that meet with much to follow, and
//! one batch of many splices. Each case prints the undo and the redo at once, so that what grows
//! with the step shows, and then crossed a slice at a time as the host runs them: the slices,
//! p50, p99 and the longest of them, the last, which commits, and the whole. What a commit lets
//! go is dropped outside the times, as the host frees it off the UI thread. Run with `-- undo`.
//!
//! Must not depend on: crate internals; it uses only the public API, and the text and timing
//! helpers of `text_perf`.

use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, Mode, Replacement};
use oso_core::editor::{Editor, ReplaceAnswer};
use oso_core::frame::Way;
use oso_core::syntax::{Highlight, Highlights};
use oso_core::window::ViewSize;

use super::{MB, editor_with_cursors, file_text, floor_boundary};

/// What else a crossing follows in a case.
#[derive(Clone, Copy)]
enum With {
    Nothing,
    Wrap,
    Window,
    Highlights,
}

/// Runs of 4 bytes over `len`, every other one a keyword, as a worker's report would be.
fn highlights(len: usize) -> Highlights {
    let runs: Vec<_> = (0..len.div_ceil(4))
        .map(|index| {
            let run = 4.min(len - index * 4);
            (run, (index % 2 == 0).then_some(Highlight::Keyword))
        })
        .collect();
    let mut highlights = Highlights::new(len);
    highlights.replace(0..len, runs);
    highlights
}

/// The undo and the redo of the editor's newest step, at once, and then a slice at a time as
/// two lines to print.
fn cross(editor: &mut Editor) -> (Duration, Duration, String) {
    let started = Instant::now();
    editor.undo().expect("a step to undo");
    let undone = started.elapsed();
    drop(editor.take_crossing_freed());
    let started = Instant::now();
    editor.redo().expect("a step to redo");
    let redone = started.elapsed();
    drop(editor.take_crossing_freed());
    let undo = sliced(editor, 1);
    let redo = sliced(editor, -1);
    let slices = format!("  undo in slices: {undo}\n  redo in slices: {redo}");
    (undone, redone, slices)
}

/// A crossing a slice at a time, as the host runs one at each turn.
struct Sliced {
    times: Vec<Duration>,
    whole: Duration,
}

impl std::fmt::Display for Sliced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let last = self.times.last().copied().unwrap_or_default();
        let mut times = self.times.clone();
        times.sort();
        let at = |share: usize| times[(times.len() - 1) * share / 100];
        write!(
            f,
            "{:>5} slices p50 {:>9.2?} p99 {:>9.2?} max {:>9.2?}  last {last:>9.2?}  whole {:>9.2?}",
            times.len(),
            at(50),
            at(99),
            at(100),
            self.whole,
        )
    }
}

/// Crosses `count` steps a slice at a time. The whole leaves out the frees.
fn sliced(editor: &mut Editor, count: isize) -> Sliced {
    let slice = Instant::now();
    editor.begin_crossing(count).expect("a step to cross");
    let mut times = vec![slice.elapsed()];
    drop(editor.take_crossing_freed());
    while editor.crossing().is_some() {
        let slice = Instant::now();
        let ended = editor.go_on_crossing();
        times.push(slice.elapsed());
        drop(editor.take_crossing_freed());
        if let Some(result) = ended {
            result.expect("the step crosses");
        }
    }
    Sliced {
        whole: times.iter().sum(),
        times,
    }
}

/// A step of `answers` runs that meet, as a replay typing forward makes: a session answered
/// `y` at each of a first line of `a`, after one command that left `edited` lines edited, with
/// `cursors` cursors in a second window. Each run that ends follows them all.
fn meeting_runs(label: &str, lines: usize, answers: usize, edited: usize, cursors: usize) {
    let text = "a".repeat(answers) + "\n" + &"line\n".repeat(lines);
    let mut editor = editor_with_cursors(&text, cursors);
    editor
        .split_window(Way::Down, true)
        .expect("the window splits");
    let id = editor.window().buffer();
    let (first, every) = (answers + 1, lines / edited * 5);
    let edits: Vec<_> = (0..edited)
        .map(|line| Replacement {
            range: first + line * every..first + line * every,
            text: "y",
        })
        .collect();
    editor.begin_undo_group();
    editor
        .apply_edits(id, &edits)
        .expect("the buffer is editable");
    editor.end_undo_group();
    editor.begin_undo_group();
    editor
        .replace_begin("a", "b", false)
        .expect("the buffer takes edits");
    editor.end_undo_group();
    // The last answer ends the session there, whose next match a worker would look for.
    for _ in 1..answers {
        editor.replace_answer(ReplaceAnswer::Replace);
    }
    editor.replace_answer(ReplaceAnswer::ReplaceAndExit);
    assert!(editor.replace().is_none(), "the session ended");
    let (undone, redone, in_slices) = cross(&mut editor);
    println!("{label:<34}       undo {undone:>9.2?}  redo {redone:>9.2?}");
    println!("{in_slices}");
}

/// One edit of `pairs` pairs of splices, each deleting a byte and replacing the next, over a
/// highlighted buffer with 1000 cursors in a second window, then its undo. The history keeps it
/// in parts of at most 4096 splices (ADR 0119).
fn one_batch(label: &str, pairs: usize) {
    let text = "ab".repeat(pairs);
    let mut editor = editor_with_cursors(&text, 1000);
    editor
        .split_window(Way::Down, true)
        .expect("the window splits");
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).expect("the buffer");
    buffer.set_mode(Mode::Rust);
    let revision = buffer.revision();
    assert!(
        buffer.take_highlights(revision, highlights(text.len())),
        "the buffer takes highlights"
    );
    let edits: Vec<_> = (0..pairs)
        .flat_map(|pair| {
            [
                Replacement {
                    range: pair * 2..pair * 2 + 1,
                    text: "",
                },
                Replacement {
                    range: pair * 2 + 1..pair * 2 + 2,
                    text: "c",
                },
            ]
        })
        .collect();
    editor.begin_undo_group();
    editor
        .apply_edits(id, &edits)
        .expect("the buffer is editable");
    editor.end_undo_group();
    let (undone, redone, in_slices) = cross(&mut editor);
    println!("{label:<34}       undo {undone:>9.2?}  redo {redone:>9.2?}");
    println!("{in_slices}");
}

/// Killing `killed` bytes from the middle of `text`, then its undo.
fn kill(label: &str, text: &str, killed: usize) {
    let start = floor_boundary(text, (text.len() - killed) / 2);
    let end = floor_boundary(text, start + killed);
    let mut editor = Editor::with_buffer(Buffer::from_text("file", text));
    let id = editor.window().buffer();
    editor.begin_undo_group();
    editor
        .apply_edits(
            id,
            &[Replacement {
                range: start..end,
                text: "",
            }],
        )
        .expect("the buffer is editable");
    editor.end_undo_group();
    let (undone, redone, in_slices) = cross(&mut editor);
    println!("{label:<34}       undo {undone:>9.2?}  redo {redone:>9.2?}");
    println!("{in_slices}");
}

/// `!` replacing `from` with `to` over `text`, with `with` turned on first, then its undo.
fn bang(label: &str, text: &str, from: &str, to: &str, with: With) {
    let mut editor = Editor::with_buffer(Buffer::from_text("file", text));
    editor.set_view_size(ViewSize::new(60, 100));
    let id = editor.window().buffer();
    match with {
        With::Nothing => {}
        With::Wrap => {
            editor.set_wraps(id, true);
        }
        With::Window => {
            editor
                .split_window(Way::Down, false)
                .expect("the window splits");
        }
        With::Highlights => {
            let buffer = editor.buffer_mut(id).expect("the buffer");
            buffer.set_mode(Mode::Rust);
            let revision = buffer.revision();
            assert!(
                buffer.take_highlights(revision, highlights(text.len())),
                "the buffer takes highlights"
            );
        }
    }
    editor.begin_undo_group();
    editor
        .replace_begin(from, to, false)
        .expect("the buffer takes edits");
    editor.end_undo_group();
    editor.replace_answer(ReplaceAnswer::All);
    let mut slices = 0;
    while editor.replace_works() {
        editor.replace_continue();
        slices += 1;
    }
    let (undone, redone, in_slices) = cross(&mut editor);
    println!("{label:<34} {slices:>5} slices: undo {undone:>9.2?}  redo {redone:>9.2?}");
    println!("{in_slices}");
}

/// A query-replace session over `text` answered `y` at each match, which makes one step of a
/// splice per answer, joined into one step when it ends, then its undo.
fn answered(label: &str, text: &str, from: &str, to: &str) {
    let mut editor = Editor::with_buffer(Buffer::from_text("file", text));
    editor.begin_undo_group();
    editor
        .replace_begin(from, to, false)
        .expect("the buffer takes edits");
    editor.end_undo_group();
    let mut answers = 0;
    while editor.replace().is_some() {
        editor.replace_answer(ReplaceAnswer::Replace);
        answers += 1;
    }
    let (undone, redone, in_slices) = cross(&mut editor);
    println!("{label:<34} {answers:>5} answers: undo {undone:>9.2?}  redo {redone:>9.2?}");
    println!("{in_slices}");
}

/// `commands` commands typing "x" at each of 1000 cursors over `text`, joined into one step as
/// self-inserts are, then its undo.
fn joined_typing(label: &str, text: &str, commands: usize) {
    let mut editor = editor_with_cursors(text, 1000);
    let id = editor.window().buffer();
    for _ in 0..commands {
        let edits: Vec<_> = editor
            .window()
            .selections
            .iter()
            .map(|selection| Replacement {
                range: selection.cursor..selection.cursor,
                text: "x",
            })
            .collect();
        editor.begin_undo_group();
        editor.allow_undo_join();
        editor
            .apply_edits(id, &edits)
            .expect("the buffer is editable");
        editor.end_undo_group();
    }
    let (undone, redone, in_slices) = cross(&mut editor);
    println!("{label:<34}       undo {undone:>9.2?}  redo {redone:>9.2?}");
    println!("{in_slices}");
}

pub(crate) fn undo_costs() {
    println!("== Undoing large steps (ADR 0119)");
    let lines = "ab\n".repeat(4_000_000);
    bang("12 MB, 4M lines of ab", &lines, "ab", "c", With::Nothing);
    bang("  in a wrapped window", &lines, "ab", "c", With::Wrap);
    bang("  in two windows", &lines, "ab", "c", With::Window);
    // Highlighting stops at 2 MiB (`syntax::MAX_BYTES`).
    let small = "ab\n".repeat(2 * MB / 3);
    bang("2 MiB, 700k lines of ab", &small, "ab", "c", With::Nothing);
    bang("  highlighted", &small, "ab", "c", With::Highlights);
    for answers in [5_000, 10_000, 20_000] {
        let sparse = "needle\n".repeat(answers) + "end\n";
        answered(
            &format!("{answers} lines, each answered y"),
            &sparse,
            "needle",
            "thread",
        );
    }
    // Answers far apart in a large file, each in a chunk of its own.
    let source = file_text(100 * MB);
    for answers in [5_000, 10_000, 20_000] {
        let mut sparse = String::with_capacity(source.len() + answers * 7);
        let every = source.len() / answers;
        let (mut placed, mut next) = (0, 0);
        for line in source.split_inclusive('\n') {
            if placed < answers && sparse.len() >= next {
                sparse.push_str("needle\n");
                placed += 1;
                next += every;
            }
            sparse.push_str(line);
        }
        answered(
            &format!("100 MB, {answers} far apart, each y"),
            &sparse,
            "needle",
            "thread",
        );
    }
    let source = "fn main() {}\n".repeat(8 * MB / 13);
    joined_typing("8 MB, 1000 cursors, 20 joined", &source, 20);
    let source = file_text(100 * MB);
    kill("89 MB killed of 100 MB", &source, 89 * MB);
    drop(source);
    meeting_runs(
        "2000 runs that meet, 200k edited",
        400_000,
        2000,
        200_000,
        1,
    );
    meeting_runs(
        "  and 100k cursors in a window",
        400_000,
        2000,
        200_000,
        100_000,
    );
    one_batch("200k splices in one edit", 100_000);
}
