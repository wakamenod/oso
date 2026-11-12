//! Measures highlighting on a large file: the first parse and highlight, in one call and in
//! the worker's slices, and the same after one key typed in the middle (ADR 0111, as built).
//!
//! Must not depend on: the editor; it calls a document directly.
//!
//! Run it in a release build, with files that are joined and cut to `oso_syntax::MAX_BYTES` at
//! a line: `cargo run --release -p oso-syntax --example syntax_perf -- rust FILE...`.
//! `fenced` makes Markdown of Rust files, every 40 lines in a fence of `rust` between two
//! paragraphs.

// A benchmark that reports to stdout, not editor code (ADR 0038).
#![expect(clippy::disallowed_macros, reason = "the benchmark prints its timings")]

use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use oso_syntax::{Document, Highlighted, Language, MAX_BYTES, Parsed, Queries};

const LONG: Duration = Duration::from_secs(60);

/// The worker's slice (ADR 0111).
const SLICE: Duration = Duration::from_millis(20);

fn main() {
    let mut args = std::env::args().skip(1);
    let (language, fenced) = match args.next().as_deref() {
        Some("rust") => (Language::Rust, false),
        Some("markdown") => (Language::Markdown, false),
        Some("fenced") => (Language::Markdown, true),
        Some("toml") => (Language::Toml, false),
        _ => return eprintln!("usage: syntax_perf rust|markdown|fenced|toml FILE..."),
    };
    let mut text = String::new();
    let files: Vec<String> = args.collect();
    while text.len() < MAX_BYTES && !files.is_empty() {
        for file in &files {
            match std::fs::read_to_string(file) {
                Ok(read) if fenced => text.push_str(&in_fences(&read)),
                Ok(read) => text.push_str(&read),
                Err(error) => return eprintln!("{file}: {error}"),
            }
        }
    }
    text.truncate(
        text[..MAX_BYTES.min(text.len())]
            .rfind('\n')
            .map_or(0, |at| at + 1),
    );
    let mut queries = Queries::new();
    let start = Instant::now();
    let mut document = Document::new(language, Rope::from(text.as_str()));
    assert_eq!(document.parse(LONG), Parsed::Done);
    let parsed = start.elapsed();
    assert_eq!(
        document.highlight(&mut queries, &[], LONG),
        Highlighted::Done
    );
    let first = start.elapsed();
    let runs = document.highlights().runs(0..text.len()).count();
    println!(
        "{} bytes, {runs} runs: parse {parsed:.1?}, parse and highlight {first:.1?}",
        text.len()
    );
    // Again in the worker's slices of 20 ms: how many, and the longest.
    let mut sliced = Document::new(language, Rope::from(text.as_str()));
    let (mut slices, mut longest) = (0, Duration::ZERO);
    loop {
        let start = Instant::now();
        let done = sliced.highlight(&mut queries, &[], SLICE);
        longest = longest.max(start.elapsed());
        slices += 1;
        if done != Highlighted::Paused {
            assert_eq!(done, Highlighted::Done);
            break;
        }
    }
    let same = sliced
        .highlights()
        .runs(0..text.len())
        .eq(document.highlights().runs(0..text.len()));
    println!("in slices of {SLICE:?}: {slices}, the longest {longest:.1?}, same runs: {same}");
    // One key in the middle, highlighted as the worker will: the shown lines first.
    let middle = text[..text.len() / 2].rfind('\n').map_or(0, |at| at + 1);
    let mut after = Rope::from(text.as_str());
    after.insert(middle, "x");
    let shown = middle..middle + 4000;
    let start = Instant::now();
    let splice = Splice {
        start: middle,
        old_len: 0,
        new_len: 1,
    };
    document.edit(&[splice], &after);
    let done = document.highlight(&mut queries, &[shown], LONG);
    assert_eq!(done, Highlighted::Done);
    println!("after one key: {:.2?}", start.elapsed());
}

/// `rust` as Markdown: every 40 of its lines in a fence, after a paragraph.
fn in_fences(rust: &str) -> String {
    let lines: Vec<&str> = rust.lines().collect();
    lines
        .chunks(40)
        .map(|chunk| {
            format!(
                "Some *text* and `code`.\n\n```rust\n{}\n```\n\n",
                chunk.join("\n")
            )
        })
        .collect()
}
