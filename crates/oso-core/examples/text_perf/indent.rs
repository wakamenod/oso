//! Indentation's rules (ADR 0115): one cursor's new line and 1000 cursors' shared pass in a
//! 2 MiB Rust file, the worst anchor search and the longest lex one line may take, and whole
//! files reindented line by line, as `indent-region` will. Run with `-- indent`.
//!
//! Must not depend on: crate internals; it uses only the public API, and the timing
//! helpers of `text_perf`.

use std::time::{Duration, Instant};

use oso_core::indent::{
    self, Answer, Budget, COMMAND_READ, LINE_COST, LINE_LOOKBACK, Line, Settings,
};
use oso_core::rope::Rope;

use super::{Lcg, MB, percentiles};

/// One item of a Rust file as rustfmt writes it, about 900 bytes.
const ITEM: &str = r#"/// What item {n} holds, with a `{` in its doc.
#[derive(Clone, Debug)]
pub struct Item {
    name: String,
    values: Vec<u32>,
}

impl Item {
    pub fn compute(&self, input: &[u32]) -> Result<u32, Error> {
        let total = input
            .iter()
            .filter(|&&x| x > 0)
            .map(|x| x * 2)
            .sum::<u32>();
        match self.values.first() {
            Some(first) if *first > total => {
                return Err(Error::new("too large: {"));
            }
            Some(_) | None => {}
        }
        /* A block comment
         * over lines. */
        let text = format!("{} {}", self.name, r"} not a brace {");
        if total > 10
            && self.name.len() > 3
            && text.chars().all(|c| c != '{')
        {
            Ok(total)
        } else {
            Ok(0)
        }
    }
}

"#;

/// Rust items to about `bytes`, each with anchors at column 0.
fn rust_file(bytes: usize) -> String {
    ITEM.repeat(bytes / ITEM.len() + 1)
}

/// One `impl` block of about `bytes`: no anchor past its first line.
fn one_impl(bytes: usize) -> String {
    let method = &ITEM[ITEM.find("    pub fn").unwrap_or(0)..ITEM.rfind("}\n\n").unwrap_or(0)];
    format!(
        "impl Big {{\n{}}}\n",
        method.repeat(bytes / method.len() + 1)
    )
}

/// The ends of `count` lines spread over `rope`, where `RET` would start a new line.
fn line_ends(rope: &Rope, count: usize) -> Vec<usize> {
    let rows = rope.line_count();
    (0..count)
        .map(|index| rope.line_end(index * rows / count))
        .collect()
}

fn ask(rope: &Rope, lines: &[Line], budget: &mut Budget) -> (Vec<Answer>, Duration) {
    let started = Instant::now();
    let answers = indent::rust(rope, Settings::default(), lines, budget);
    (answers, started.elapsed())
}

/// p50 and p99 of `RET` at every one of `lines` in `rope` in one command, over `runs` runs,
/// and whether the budget ran out.
fn shared(rope: &Rope, lines: &[Line], runs: usize) -> (Duration, Duration, bool) {
    let mut spent = false;
    let mut samples: Vec<Duration> = (0..runs)
        .map(|_| {
            let mut budget = Budget::new();
            let (_, elapsed) = ask(rope, lines, &mut budget);
            spent |= budget.is_spent();
            elapsed
        })
        .collect();
    let (p50, p99) = percentiles(&mut samples);
    (p50, p99, spent)
}

/// One cursor and many in a file of items as rustfmt writes them.
fn cursor_costs() {
    let rope = Rope::from(rust_file(2 * MB).as_str());
    let rows = rope.line_count();
    let mut rng = Lcg(17);
    let mut samples: Vec<Duration> = (0..5000)
        .map(|_| {
            let at = rope.line_end(rng.below(rows));
            ask(&rope, &[Line::New(at)], &mut Budget::new()).1
        })
        .collect();
    let (p50, p99) = percentiles(&mut samples);
    let most = samples.iter().max().copied().unwrap_or_default();
    println!("2 MiB Rust, RET at 1 cursor     p50 {p50:>9.2?}  p99 {p99:>9.2?}  max {most:>9.2?}");
    for cursors in [10, 100, 1000] {
        let lines: Vec<_> = line_ends(&rope, cursors)
            .into_iter()
            .map(Line::New)
            .collect();
        let (p50, p99, spent) = shared(&rope, &lines, 20);
        let mut unbounded = Budget::with_limit(usize::MAX);
        let (_, alone) = ask(&rope, &lines, &mut unbounded);
        println!(
            "2 MiB Rust, RET at {cursors:>4} cursors p50 {p50:>9.2?}  p99 {p99:>9.2?}  spent {spent}  (unbounded {alone:.2?})"
        );
    }
}

/// The worst cases: one `impl` with no anchor past its first line, and Markdown, whose only
/// anchor is the buffer's start.
fn worst_costs() {
    let big = Rope::from(one_impl(2 * MB).as_str());
    let lines: Vec<_> = line_ends(&big, 1000).into_iter().map(Line::New).collect();
    let (p50, p99, spent) = shared(&big, &lines, 20);
    println!("2 MiB impl, RET at 1000 cursors p50 {p50:>9.2?}  p99 {p99:>9.2?}  spent {spent}");
    let worst_lex = big.line_end(big.offset_to_point(LINE_LOOKBACK - 64).row);
    let no_anchor = big.line_end(big.offset_to_point(big.len() / 2).row);
    for (label, at) in [
        ("lex from the reach", worst_lex),
        ("no anchor in reach", no_anchor),
    ] {
        let (p50, p99, _) = shared(&big, &[Line::New(at)], 200);
        println!("2 MiB impl, {label:<20} p50 {p50:>9.2?}  p99 {p99:>9.2?}");
    }
    let notes = "## A heading\n\n- an item\n  - a nested item with some text\n    more text\n\n```rust\nfn main() {}\n```\n\nA paragraph of prose.\n";
    let notes = Rope::from(notes.repeat(2 * MB / notes.len()).as_str());
    let far = notes.line_end(notes.offset_to_point(LINE_LOOKBACK - 64).row);
    let mut samples: Vec<Duration> = (0..200)
        .map(|_| {
            let started = Instant::now();
            let lines = [Line::New(far)];
            indent::markdown(&notes, Settings::default(), &lines, &mut Budget::new());
            started.elapsed()
        })
        .collect();
    let (p50, p99) = percentiles(&mut samples);
    println!("2 MiB Markdown, RET at the reach p50 {p50:>9.2?}  p99 {p99:>9.2?}");
}

/// Crafted text: a line of block comments, runs of short lines, and lines that go on.
fn crafted_costs() {
    let comments = format!("fn f() {{\n    x\t日{}\n}}\n", "/**/".repeat(16_000));
    let comments = Rope::from(comments.as_str());
    let (p50, p99, _) = shared(&comments, &[Line::New(comments.line_end(1))], 50);
    println!("64 KiB line of /**/, RET after   p50 {p50:>9.2?}  p99 {p99:>9.2?}");
    let blank = Rope::from(format!("fn f() {{\n{}", "\n".repeat(LINE_LOOKBACK)).as_str());
    let (p50, p99, _) = shared(&blank, &[Line::New(blank.len())], 50);
    println!("64 KiB of blank lines, RET after p50 {p50:>9.2?}  p99 {p99:>9.2?}");
    for (label, line) in [("blank", "\n"), ("x", "x\n"), ("{", "{\n")] {
        let rope = Rope::from(line.repeat(2 * MB / line.len()).as_str());
        let lines: Vec<_> = line_ends(&rope, 1000).into_iter().map(Line::New).collect();
        let (p50, p99, spent) = shared(&rope, &lines, 20);
        println!(
            "2 MiB of {label:<5} lines, 1000 cursors p50 {p50:>9.2?}  p99 {p99:>9.2?}  spent {spent}"
        );
    }
    let lines_of_x = Rope::from(format!("fn f() {{\n{}}}\n", "x\n".repeat(100_000)).as_str());
    let rows: Vec<_> = (0..lines_of_x.line_count()).map(Line::Row).collect();
    let answers = ask(&lines_of_x, &rows, &mut Budget::with_limit(usize::MAX)).0;
    let deepest = answers
        .iter()
        .filter_map(|answer| match answer {
            Answer::Column(column) => Some(*column),
            Answer::Keep => None,
        })
        .max();
    println!("100K lines of x, every line: deepest column {deepest:?}");
}

pub(crate) fn indent_costs() {
    println!("== Indentation rules (ADR 0115)");
    println!(
        "line reach {} KiB, command budget {} KiB",
        LINE_LOOKBACK >> 10,
        COMMAND_READ >> 10
    );
    cursor_costs();
    worst_costs();
    crafted_costs();
    // Every line, as `indent-region` asks.
    for size in [2 * MB, 4 * MB, 8 * MB] {
        let rope = Rope::from(rust_file(size).as_str());
        let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
        // The budget a region of this size needs, as step 3 will give it.
        let needed = size + lines.len() * LINE_COST + LINE_LOOKBACK;
        let mut spent = false;
        let mut samples: Vec<Duration> = (0..5)
            .map(|_| {
                let mut budget = Budget::with_limit(needed);
                let elapsed = ask(&rope, &lines, &mut budget).1;
                spent |= budget.is_spent();
                elapsed
            })
            .collect();
        let (p50, _) = percentiles(&mut samples);
        let rate = size as f64 / MB as f64 / p50.as_secs_f64();
        println!(
            "{:>2} MiB Rust, every line          p50 {p50:>9.2?}  ({rate:.0} MiB/s, {} lines, spent {spent})",
            size / MB,
            lines.len()
        );
    }
}
