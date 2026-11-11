//! Following edits in a buffer's highlights: typing at one cursor and at a thousand, and one
//! batch of 100,000 splices, on a map of 2 MiB with a run every few bytes (ADR 0091, ADR 0111).
//! Run with `-- highlights`.
//!
//! Must not depend on: crate internals; it uses only the public API, and the timing helpers of
//! `text_perf`.

use std::time::{Duration, Instant};

use oso_core::syntax::{Highlight, Highlights, Splice};

use super::{MB, percentiles};

/// 2 MiB of runs of 3 to 8 bytes, as highlighted code has, one run in three plain.
fn map() -> Highlights {
    let kinds = [None, Some(Highlight::Keyword), Some(Highlight::String)];
    let len = 2 * MB;
    let mut runs = Vec::new();
    let (mut at, mut index) = (0, 0usize);
    while at < len {
        let run = 3 + index % 6;
        runs.push((run, kinds[index % kinds.len()]));
        at += run;
        index += 1;
    }
    let mut highlights = Highlights::new(len);
    highlights.replace(0..len, runs);
    highlights
}

/// One command's batch: a character typed at each of `cursors` cursors spread over `len` bytes.
fn typed(len: usize, cursors: usize, round: usize) -> Vec<Splice> {
    let stride = len / cursors;
    (0..cursors)
        .map(|index| Splice {
            start: index * (stride + 1) + round % stride.max(1),
            old_len: 0,
            new_len: 1,
        })
        .collect()
}

pub(super) fn highlight_costs() {
    let base = map();
    for cursors in [1, 1000] {
        let commands = if cursors == 1 { 10_000 } else { 300 };
        let mut highlights = base.clone();
        let mut samples: Vec<Duration> = Vec::with_capacity(commands);
        for round in 0..commands {
            let batch = typed(highlights.len(), cursors, round);
            // As an edit stages: on a clone, while the committed map still shares every node.
            let start = Instant::now();
            let mut staged = highlights.clone();
            staged.splice(&batch);
            samples.push(start.elapsed());
            highlights = staged;
        }
        let (p50, p99) = percentiles(&mut samples);
        println!("highlights: typing at {cursors:>4} cursors, 2 MiB: p50 {p50:?}, p99 {p99:?}");
    }
    let mut highlights = base.clone();
    let shared = base.clone();
    let batch: Vec<Splice> = (0..100_000)
        .map(|index| Splice {
            start: index * 20,
            old_len: 1,
            new_len: 2,
        })
        .collect();
    let start = Instant::now();
    highlights.splice(&batch);
    println!(
        "highlights: one batch of 100,000 splices, 2 MiB: {:?}",
        start.elapsed()
    );
    drop(shared);
}
