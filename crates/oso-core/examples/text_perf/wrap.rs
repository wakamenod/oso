//! Timings of soft wrap (ADR 0100): wrapping large buffers and long lines, and the rewrap after
//! typing in a wrapped 10 MB line, at one cursor and at a thousand.
//!
//! Must not depend on: other workspace crates or crate internals; it uses only the public API.

use std::hint::black_box;
use std::time::Instant;

use oso_core::display_map::wrap_map::{COMMAND_BUDGET, Splice, WrapSnapshot};
use oso_core::display_map::{DisplayMap, TabWidth};
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;

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
}
