//! Timings of soft wrap (ADR 0100): wrapping large buffers and long lines, and the rewrap after
//! typing in a wrapped 10 MB line, at one cursor and at a thousand.
//!
//! Must not depend on: other workspace crates or crate internals; it uses only the public API.

use std::hint::black_box;
use std::time::Instant;

use oso_core::display_map::wrap_map::WrapSnapshot;
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

/// Times wrapping `text` from scratch, and says how many rows it makes.
fn build(label: &str, text: &str) {
    let rope = Rope::from(text);
    let display = DisplayMap::new(&rope, TabWidth::new(4)).snapshot();
    let (wrap, elapsed) = time(|| WrapSnapshot::new(&display, COLUMNS));
    println!(
        "wrap {label:<24} {elapsed:>10.2?}  {:>9} rows",
        black_box(&wrap).row_count()
    );
}

/// Times typing in `text`, a wrapped line, at `cursors` places: each key is one splice per
/// cursor, the last first, as the editor applies them, each followed by its rewrap.
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
        for &place in places.iter().rev() {
            let edit = rope.insert(place, "x");
            let before = map.snapshot();
            map.sync(&rope, &edit);
            wrap = wrap.edited(&before, &map.snapshot(), &edit);
        }
        samples.push(start.elapsed());
    }
    let (p50, p99) = percentiles(&mut samples);
    println!("type {label:<24} {cursors:>5} cursors  p50 {p50:>9.2?} p99 {p99:>9.2?}");
}

/// Every timing of soft wrap.
pub(super) fn wrap_costs() {
    println!("== Soft wrap at {COLUMNS} columns");
    build("100 MB of source", &file_text(100 * MB));
    build("100 MB of prose", &prose(100 * MB));
    let lines = [
        ("10 MB line, words", line_of("word and more ", 10 * MB)),
        ("10 MB line, CJK", line_of("日本語の文章、", 10 * MB)),
        ("10 MB line, no spaces", line_of("abcdefghij", 10 * MB)),
    ];
    for (label, text) in &lines {
        build(label, text);
    }
    for (label, text) in &lines {
        typing(label, text, 1);
    }
    // In CJK or with no spaces, rows after an edit may not resync before the line's end, and
    // until the UI thread's budget and the worker take that tail (ADR 0100), a thousand cursors
    // there cost a thousand lines' rewraps.
    typing(lines[0].0, &lines[0].1, 1000);
}
