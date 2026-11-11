//! Timings of soft wrap (ADR 0100): wrapping large buffers and long lines, the rewrap after
//! typing in a wrapped 10 MB line, at one cursor and at a thousand, and what a window does on
//! the UI thread when wrapping turns on, its width changes or a command types (ADR 0103), and
//! building the rows a frame shows (ADR 0104).
//!
//! Must not depend on: other workspace crates or crate internals; it uses only the public API.

use std::hint::black_box;
use std::time::Instant;

use oso_core::buffer::{Buffer, Replacement};
use oso_core::display_map::wrap_map::{COMMAND_BUDGET, Splice, WrapSnapshot};
use oso_core::display_map::{DisplayMap, Preedit, TabWidth};
use oso_core::editor::Editor;
use oso_core::rope::Rope;
use oso_core::selection::{Selection, SelectionSet};
use oso_core::sum_tree::Bias;
use oso_core::window::{Screen, Scroll, ViewSize};

use super::{Lcg, MB, file_text, percentiles, time};

/// The window's width in cells.
const COLUMNS: usize = 100;

/// About `bytes` of prose: paragraphs of English and of Japanese on long lines.
fn prose(bytes: usize) -> String {
    let english = "The quick brown fox jumps over the lazy dog, and then it rests a while. ";
    let japanese = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。";
    let mut text = String::with_capacity(bytes + 1024);
    while text.len() < bytes {
        for _ in 0..12 {
            text.push_str(english);
        }
        text.push('\n');
        for _ in 0..12 {
            text.push_str(japanese);
        }
        text.push_str("\n\n");
    }
    text
}

/// One line of about `bytes`, repeating `piece`.
fn line_of(piece: &str, bytes: usize) -> String {
    piece.repeat(bytes / piece.len())
}

/// Times wrapping `text` from scratch at `columns`, and says how many rows it makes, how many
/// of them continue a line, and the memory they take.
fn build(label: &str, text: &str, columns: usize) {
    let rope = Rope::from(text);
    let display = DisplayMap::new(&rope, TabWidth::new(4)).snapshot();
    let (wrap, elapsed) = time(|| WrapSnapshot::new(&display, columns));
    let rows = black_box(&wrap).row_count();
    let continuing = rows - display.row_count();
    let bytes = wrap.heap_bytes();
    let per = |count: usize| bytes as f64 / count.max(1) as f64;
    println!(
        "wrap {label:<22} {columns:>3} cols {elapsed:>9.2?} {rows:>9} rows {continuing:>8} continuing  {:>7.1} MB  {:>5.1} B/continuing {:>4.1} B/row",
        bytes as f64 / MB as f64,
        per(continuing),
        per(rows),
    );
}

/// Times typing in `text`, a wrapped line, at `cursors` places: each key is one splice per
/// cursor, the last first, as the editor applies them, each followed by its rewrap within the
/// command's budget, as a window's is; what that leaves stale goes to a worker.
fn typing(label: &str, text: &str, cursors: usize) {
    let mut rope = Rope::from(text);
    let mut map = DisplayMap::new(&rope, TabWidth::new(4));
    let mut wrap = WrapSnapshot::new(&map.snapshot(), COLUMNS);
    let mut rng = Lcg(7);
    let keys = (200 / cursors).clamp(3, 200);
    let mut samples = Vec::with_capacity(keys);
    for _ in 0..keys {
        let mut places: Vec<usize> = (0..cursors)
            .map(|_| rope.clip_offset(rng.below(rope.len()), Bias::Left))
            .collect();
        places.sort_unstable();
        places.dedup();
        let start = Instant::now();
        let mut splices = Vec::with_capacity(places.len());
        for &place in places.iter().rev() {
            let edit = rope.insert(place, "x");
            let before = map.snapshot();
            map.sync(&rope, &edit);
            splices.push(Splice {
                before,
                after: map.snapshot(),
                edit,
            });
        }
        let mut budget = COMMAND_BUDGET;
        wrap = wrap.edited_command(&splices, &mut budget).0;
        samples.push(start.elapsed());
    }
    let (p50, p99) = percentiles(&mut samples);
    println!("type {label:<24} {cursors:>5} cursors  p50 {p50:>9.2?} p99 {p99:>9.2?}");
}

/// One line of about `bytes` of words of mixed lengths, English or Japanese as `cjk` says, so
/// that a row's breaks shifted by an edit meet the old ones again soon, as in prose. A line that
/// repeats one short piece may never meet them, and leaves its rest to the worker.
fn varied_line(bytes: usize, cjk: bool) -> String {
    let words: &[&str] = if cjk {
        &[
            "吾輩は",
            "猫である。",
            "名前は",
            "まだ無い。",
            "どこで",
            "生れたか",
            "とんと",
            "見当が",
            "つかぬ。",
            "、",
        ]
    } else {
        &[
            "a", "word", "and", "more", "quickly", "the", "jumps", "over", "lazy", "dog,", "then",
        ]
    };
    let mut rng = Lcg(5);
    let mut line = String::with_capacity(bytes + 32);
    while line.len() < bytes {
        line.push_str(words[rng.below(words.len())]);
        if !cjk {
            line.push(' ');
        }
    }
    line
}

/// An editor 60 rows by `COLUMNS` on `text`, its cursor at `cursor` and in view.
fn window_on(text: &str, cursor: usize) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("perf", text));
    editor.set_view_size(ViewSize::new(60, COLUMNS));
    editor.restore_positions(
        SelectionSet::new(cursor),
        Scroll {
            start: cursor,
            left: 0,
        },
    );
    editor
}

/// Times what the UI thread does when wrapping turns on in a window on `text` with its cursor
/// at `cursor`, and when the window then narrows: the rows in view, never the buffer.
fn turning_on(label: &str, text: &str, cursor: usize) {
    let mut editor = window_on(text, cursor);
    let id = editor.window().buffer();
    editor.begin_undo_group();
    let (_, on) = time(|| editor.set_wraps(id, true));
    editor.end_undo_group();
    let (_, narrower) = time(|| editor.set_view_size(ViewSize::new(60, COLUMNS - 20)));
    let jobs = editor.take_wrap_jobs().len();
    println!("window {label:<29} on {on:>9.2?}  narrower {narrower:>9.2?}  {jobs} job");
}

/// Times commands that type at `cursors` places in a window on `text`, wrapped or not, as the
/// dispatcher runs them: the edit, each window's rewrap and following the cursor. The places
/// are random, and each key types at every cursor, where the one before left it. No job runs
/// between them, but with `settled`, the jobs of turning wrapping on run first, as the worker
/// would have.
fn window_typing(label: &str, text: &str, cursors: usize, (wraps, settled): (bool, bool)) {
    let mut editor = window_on(text, text.len() / 2);
    let id = editor.window().buffer();
    let mut rng = Lcg(11);
    let rope = editor.current().1.text().clone();
    let mut places: Vec<usize> = (0..cursors)
        .map(|_| rope.clip_offset(rng.below(rope.len()), Bias::Left))
        .collect();
    places.sort_unstable();
    places.dedup();
    let selections = places.iter().copied().map(Selection::at).collect();
    let set = SelectionSet::from_parts(selections, places.len() / 2).unwrap_or_default();
    editor.restore_positions(
        set,
        Scroll {
            start: places[places.len() / 2],
            left: 0,
        },
    );
    editor.set_wraps(id, wraps);
    if settled {
        for job in editor.take_wrap_jobs() {
            if let Some(report) = job.run() {
                editor.take_wrap_report(report);
            }
        }
    }
    let keys = (200 / cursors).clamp(3, 200);
    let mut samples = Vec::with_capacity(keys);
    for _ in 0..keys {
        let start = Instant::now();
        editor.begin_undo_group();
        let replacements: Vec<Replacement<'_>> = (editor.window().selections.iter())
            .map(|selection| Replacement {
                range: selection.cursor..selection.cursor,
                text: "x",
            })
            .collect();
        let _ = editor.apply_edits(id, &replacements);
        editor.end_undo_group();
        editor.follow_cursor();
        samples.push(start.elapsed());
    }
    let (p50, p99) = percentiles(&mut samples);
    let how = match (wraps, settled) {
        (false, _) => "truncated",
        (true, false) => "unsettled",
        (true, true) => "wrapped",
    };
    println!("window {label:<22} {how:>9} {cursors:>5} cursors  p50 {p50:>9.2?} p99 {p99:>9.2?}");
}

/// Times undoing 20 keys typed at `cursors` places, joined in one undo group, in a window on
/// `text`, wrapped or not: one command of 20 times as many splices.
fn window_undo(label: &str, text: &str, cursors: usize, wraps: bool) {
    let mut editor = window_on(text, text.len() / 2);
    let id = editor.window().buffer();
    let mut rng = Lcg(13);
    let rope = editor.current().1.text().clone();
    let mut places: Vec<usize> = (0..cursors)
        .map(|_| rope.clip_offset(rng.below(rope.len()), Bias::Left))
        .collect();
    places.sort_unstable();
    places.dedup();
    let selections = places.iter().copied().map(Selection::at).collect();
    let set = SelectionSet::from_parts(selections, places.len() / 2).unwrap_or_default();
    editor.restore_positions(set, Scroll::default());
    editor.set_wraps(id, wraps);
    for job in editor.take_wrap_jobs() {
        if let Some(report) = job.run() {
            editor.take_wrap_report(report);
        }
    }
    for _ in 0..20 {
        editor.begin_undo_group();
        editor.allow_undo_join();
        let replacements: Vec<Replacement<'_>> = (editor.window().selections.iter())
            .map(|selection| Replacement {
                range: selection.cursor..selection.cursor,
                text: "x",
            })
            .collect();
        let _ = editor.apply_edits(id, &replacements);
        editor.end_undo_group();
    }
    editor.begin_undo_group();
    let (_, undone) = time(|| editor.undo());
    editor.end_undo_group();
    let how = if wraps { "wrapped" } else { "truncated" };
    println!("undo   {label:<22} {how:>9} {cursors:>5} cursors x 20 keys  {undone:>9.2?}");
}

/// Times building the rows a frame shows of a window on `text`, its cursor at `cursor`:
/// truncated, wrapped once the jobs have reported, and wrapped with a preedit laid out in the
/// cursor's line (ADR 0104). Once a frame, so it must stay well under a frame's budget.
fn screen_rows(label: &str, text: &str, cursor: usize) {
    let mut editor = window_on(text, cursor);
    let id = editor.window().buffer();
    let mut screen = Screen::default();
    let mut frames = |editor: &Editor| {
        let mut samples: Vec<_> = (0..200)
            .map(|_| time(|| editor.screen(black_box(&mut screen))).1)
            .collect();
        percentiles(&mut samples)
    };
    let (truncated, _) = frames(&editor);
    editor.set_wraps(id, true);
    for job in editor.take_wrap_jobs() {
        if let Some(report) = job.run() {
            editor.take_wrap_report(report);
        }
    }
    editor.follow_cursor();
    let (p50, p99) = frames(&editor);
    editor.set_preedit(Some(Preedit::new("日本語の入力中の文".repeat(4), None)));
    let (composing, composing_p99) = frames(&editor);
    println!(
        "screen {label:<29} truncated {truncated:>9.2?}  wrapped p50 {p50:>9.2?} p99 {p99:>9.2?}  \
         composing p50 {composing:>9.2?} p99 {composing_p99:>9.2?}"
    );
}

/// The timings of the rows a frame shows, alone (`text_perf -- screen`), and after the rest of
/// soft wrap's.
pub(super) fn screen_costs() {
    let lines = [
        ("10 MB line, words", line_of("word and more ", 10 * MB)),
        ("10 MB line, CJK", line_of("日本語の文章、", 10 * MB)),
        ("10 MB line, no spaces", line_of("abcdefghij", 10 * MB)),
        ("100 MB of source", file_text(100 * MB)),
    ];
    for (label, text) in &lines {
        screen_rows(&format!("{label}, mid"), text, text.len() / 2);
    }
}

/// Every timing of soft wrap; with `1g`, wrapping 1 GB of source too.
pub(super) fn wrap_costs() {
    println!("== Soft wrap");
    let source = file_text(100 * MB);
    let prose = prose(100 * MB);
    for columns in [40, 80, COLUMNS] {
        build("100 MB of source", &source, columns);
        build("100 MB of prose", &prose, columns);
    }
    drop((source, prose));
    if std::env::args().any(|arg| arg == "1g") {
        build("1 GB of source", &file_text(1024 * MB), 80);
    }
    let lines = [
        ("10 MB line, words", line_of("word and more ", 10 * MB)),
        ("10 MB line, CJK", line_of("日本語の文章、", 10 * MB)),
        ("10 MB line, no spaces", line_of("abcdefghij", 10 * MB)),
        // A tab far after every edit, whose cells depend on the column there.
        (
            "10 MB line, words, tab",
            line_of("word and more ", 10 * MB) + "\t.",
        ),
    ];
    for (label, text) in &lines {
        build(label, text, COLUMNS);
    }
    for (label, text) in &lines {
        for cursors in [1, 1000] {
            typing(label, text, cursors);
        }
    }
    // Commands one after another, no job between, on a line ten times as long: what a
    // provisional row costs must not grow with it.
    let long = line_of("word and more ", 100 * MB);
    typing("100 MB line, words", &long, 1000);
    drop(long);
    println!();
    let prose = self::prose(100 * MB);
    turning_on("100 MB of prose, at its end", &prose, prose.len() - 1);
    drop(prose);
    for (label, text) in &lines {
        turning_on(&format!("{label}, mid"), text, text.len() / 2);
    }
    let varied = [
        ("10 MB line, words", varied_line(10 * MB, false)),
        ("10 MB line, CJK", varied_line(10 * MB, true)),
    ];
    for (label, text) in &varied {
        for cursors in [1, 1000] {
            window_typing(label, text, cursors, (false, false));
            window_typing(label, text, cursors, (true, true));
            window_typing(label, text, cursors, (true, false));
        }
        window_undo(label, text, 1000, false);
        window_undo(label, text, 1000, true);
    }
    println!();
    screen_costs();
}
