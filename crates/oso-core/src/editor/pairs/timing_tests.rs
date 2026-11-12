//! Timings of moving a pair of windows in step, run by hand in a release build (ADR 0122,
//! step 10).
//!
//! Must not depend on: the host's window.

use super::tests::{on, selected, window};
use super::*;
use crate::buffer::Buffer;
use crate::selection::SelectionSet;

/// What moving in step costs at a command's end, over two texts of about 1 GB and a map of a
/// million runs, against the same two windows unpaired. Run it by hand in release:
/// `cargo test --release -p oso-core moving_in_step_costs -- --ignored --nocapture`.
#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(
    clippy::disallowed_macros,
    reason = "a timing run by hand prints what it measured"
)]
fn moving_in_step_costs_little_at_a_commands_end() {
    use std::time::{Duration, Instant};
    const LINES: usize = 16 << 20;
    let text: String = (0..LINES)
        .map(|i| format!("{i:08} {}\n", "x".repeat(54)))
        .collect();
    let runs: Vec<(usize, usize, usize)> =
        (0..1 << 20).map(|i| (i * 16, i * 16 + i % 2, 15)).collect();
    // Each within a tenth of the 8 ms a key may take to show, at p99, in a release build.
    let percentiles = |mut times: Vec<Duration>| {
        times.sort();
        let at = |p: usize| times[(times.len() - 1) * p / 100];
        assert!(cfg!(debug_assertions) || at(99) < Duration::from_micros(800));
        format!("p50 {:?} p99 {:?} max {:?}", at(50), at(99), at(100))
    };
    let line = text.len() / LINES;
    let mut seed: usize = 7;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % (LINES - 1)
    };
    for paired in [false, true] {
        let mut editor = on(&text, (60, 241));
        let buffer = editor.add_buffer(Buffer::from_text("other", &text));
        if paired {
            let started = Instant::now();
            let map = RowMap::new(runs.clone()).unwrap();
            editor.pair_window(Way::Across, buffer, map).unwrap();
            eprintln!("pairing with a map of 2^20 runs: {:?}", started.elapsed());
            let started = Instant::now();
            let first = selected(&editor);
            editor.set_row_map(first, RowMap::new(runs.clone()).unwrap());
            eprintln!("a new map, its runs built: {:?}", started.elapsed());
        } else {
            editor.split_window(Way::Across, false).unwrap();
            let other = editor.pair_of(selected(&editor));
            assert_eq!(other, None);
        }
        let mut moves = Vec::new();
        let mut steps = Vec::new();
        for i in 0..2000 {
            // Far jumps and single lines, as goto-line and C-n do.
            let target = if i % 2 == 0 { next() } else { 0 };
            let offset = if target == 0 {
                let at = editor.window().selections.primary().cursor;
                (at + line).min(text.len() - 1)
            } else {
                target * line + 3
            };
            editor.current_mut().0.selections = SelectionSet::new(offset);
            let started = Instant::now();
            editor.follow_cursor();
            let took = started.elapsed();
            if target == 0 {
                steps.push(took)
            } else {
                moves.push(took)
            }
        }
        let mut wheel = Vec::new();
        let id = selected(&editor);
        for i in 0..500 {
            let started = Instant::now();
            editor.scroll_view(id, if i % 2 == 0 { 3 } else { -2 });
            wheel.push(started.elapsed());
        }
        let name = if paired { "paired" } else { "unpaired" };
        eprintln!("{name}: a far move {}", percentiles(moves));
        eprintln!("{name}: a line down {}", percentiles(steps));
        eprintln!("{name}: a wheel turn {}", percentiles(wheel));
    }
}

/// What moving in step costs on a 10 MB line both windows wrap: a character at a time from its
/// middle, and the wheel. Run it by hand in release:
/// `cargo test --release -p oso-core moving_in_step_on_a_long -- --ignored --nocapture`.
#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(
    clippy::disallowed_macros,
    reason = "a timing run by hand prints what it measured"
)]
fn moving_in_step_on_a_long_wrapped_line_costs_little() {
    use std::time::{Duration, Instant};
    let text = "word ".repeat(2 << 20) + "\n";
    let percentiles = |mut times: Vec<Duration>| {
        times.sort();
        let at = |p: usize| times[(times.len() - 1) * p / 100];
        assert!(cfg!(debug_assertions) || at(99) < Duration::from_micros(800));
        format!("p50 {:?} p99 {:?} max {:?}", at(50), at(99), at(100))
    };
    for paired in [false, true] {
        let mut editor = on(&text, (40, 161));
        let first_buffer = editor.window().buffer();
        let buffer = editor.add_buffer(Buffer::from_text("other", &text));
        let other = if paired {
            editor
                .pair_window(Way::Across, buffer, RowMap::same())
                .unwrap()
        } else {
            let other = editor.split_window(Way::Across, false).unwrap();
            editor.show_in(other, buffer);
            other
        };
        assert!(editor.set_wraps(first_buffer, true) && editor.set_wraps(buffer, true));
        let mut keys = Vec::new();
        let mut moved = 0;
        let mut at = text.len() / 2;
        for _ in 0..2000 {
            at += 1;
            editor.refill_wraps();
            editor.current_mut().0.selections = SelectionSet::new(at);
            let before = window(&editor, other).scroll();
            let started = Instant::now();
            editor.follow_cursor();
            keys.push(started.elapsed());
            moved += usize::from(window(&editor, other).scroll() != before);
        }
        let mut wheel = Vec::new();
        let id = selected(&editor);
        for i in 0..500 {
            editor.refill_wraps();
            let started = Instant::now();
            editor.scroll_view(id, if i % 2 == 0 { 3 } else { -2 });
            wheel.push(started.elapsed());
        }
        let name = if paired { "paired" } else { "unpaired" };
        eprintln!(
            "{name}: a character on, wrapped {} (the other moved {moved} times in 2000)",
            percentiles(keys)
        );
        eprintln!("{name}: a wheel turn, wrapped {}", percentiles(wheel));
    }
}
