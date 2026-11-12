//! Crafted nesting and random text for every grammar: texts made of a few openers repeated, or
//! of any characters, highlighted on a thread with a small stack (ADR 0111). The openers are
//! combined here, so no case is written out; deeply nested inline links are among the
//! combinations.
//!
//! Must not depend on: the worker; these call a document directly.
//!
//! - Markdown's inline atoms make texts far longer than what overflows the test's stack in its
//!   inline grammar, so a missing cap aborts the test. The other cases are shorter: a query
//!   over a node of many children costs the square of their number in tree-sitter, so a long
//!   one would only spend the test's time.
//! - The property test reads its number of cases from `PROPTEST_CASES`, so that the weekly
//!   workflow runs many more.

use std::time::Duration;

use oso_core::rope::Rope;
use proptest::prelude::*;

use super::inline::MAX_INLINE_BYTES;
use super::tests::{LONG, cases};
use super::{Document, Failure, Highlighted};
use crate::language::Language;
use crate::queries::Queries;

/// What the texts are made of: openers and fillers of each grammar.
const ATOMS: [&str; 20] = [
    "[",
    "a",
    "](",
    "(",
    "*",
    "_",
    "`",
    "<",
    "!",
    "> ",
    "- ",
    "1. ",
    "{",
    "\"",
    "/*",
    "#[",
    "x=",
    "\n",
    "```rust\n",
    "~~~toml\n",
];

/// The stack of the thread the texts are highlighted on: an eighth of the 8 MiB step 4 gives
/// the worker, and twice what the inline cap needs.
const STACK: usize = 1 << 20;

/// How long an inline case is: past what overflows `STACK` without the inline cap.
const INLINE_BYTES: usize = 64 << 10;

/// How long another case is.
const BYTES: usize = 16 << 10;

/// The worker's slice (ADR 0111).
const SLICE: Duration = Duration::from_millis(20);

const LANGUAGES: [Language; 3] = [Language::Rust, Language::Markdown, Language::Toml];

/// Highlights `text` to the end in slices of `slice`, on a thread with a stack of [`STACK`], and
/// checks what came of it.
fn highlight_on_a_small_stack(
    language: Language,
    text: String,
    slice: Duration,
) -> Result<(), String> {
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let mut document = Document::new(language, Rope::from(text.as_str()));
            let mut queries = Queries::new();
            // A slice leaves plain what outlasts it, so each gets somewhere.
            for _ in 0..10_000 {
                match document.highlight(&mut queries, &[], slice) {
                    Highlighted::Paused => continue,
                    Highlighted::Done => break,
                    // Too slow for a slice: plain, which the checks below still hold for.
                    Highlighted::Failed(Failure::TooSlow) => break,
                    failed => return Err(format!("{language:?}: {failed:?}")),
                }
            }
            let done = document.highlight(&mut queries, &[], slice);
            if !matches!(
                done,
                Highlighted::Done | Highlighted::Failed(Failure::TooSlow)
            ) {
                return Err(format!("{language:?}: {done:?}"));
            }
            let highlights = document.highlights();
            if highlights.len() != text.len() {
                return Err(format!(
                    "{language:?}: {} of {}",
                    highlights.len(),
                    text.len()
                ));
            }
            match text
                .match_indices('\n')
                .find(|(at, _)| highlights.at(*at).is_some())
            {
                Some((at, _)) => Err(format!("{language:?}: line break at {at} highlighted")),
                None => Ok(()),
            }
        })
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| format!("{language:?} panicked"))?
}

fn repeated(atoms: &[usize], bytes: usize) -> String {
    let unit: String = atoms.iter().map(|&atom| ATOMS[atom]).collect();
    unit.repeat(bytes / unit.len() + 1)
}

/// Three of Markdown's inline atoms, in every order three at once.
#[test]
fn every_three_inline_atoms_nest_safely() {
    let inline = [0, 1, 2];
    for &a in &inline {
        for &b in &inline {
            for &c in &inline {
                // One slice, so that each parse ends: a short one would stop it first.
                let text = repeated(&[a, b, c], INLINE_BYTES);
                highlight_on_a_small_stack(Language::Markdown, text, LONG).unwrap();
            }
        }
    }
}

/// The same three atoms just under the inline cap, which the inline grammar parses: what the
/// cap lets through fits a stack far smaller than the worker's.
#[test]
fn every_three_inline_atoms_under_the_cap_fit_the_stack() {
    let inline = [0, 1, 2];
    for &a in &inline {
        for &b in &inline {
            for &c in &inline {
                let mut text = repeated(&[a, b, c], MAX_INLINE_BYTES);
                text.truncate(MAX_INLINE_BYTES - 1);
                highlight_on_a_small_stack(Language::Markdown, text, LONG).unwrap();
            }
        }
    }
}

/// A crafted text no piece of which fits a slice fails as too slow, and stays plain, in a few
/// slices: pieces halve from 16 KiB to 1 KiB first.
#[test]
fn a_text_too_slow_for_a_slice_fails_in_a_few_slices() {
    let text = repeated(&[3], 64 << 10);
    let mut document = Document::new(Language::Rust, Rope::from(text.as_str()));
    let mut queries = Queries::new();
    let mut calls = 0;
    let started = std::time::Instant::now();
    let done = loop {
        calls += 1;
        match document.highlight(&mut queries, &[], SLICE) {
            Highlighted::Paused if calls < 100 => continue,
            done => break done,
        }
    };
    assert_eq!(done, Highlighted::Failed(Failure::TooSlow));
    assert!(calls <= 8, "{calls} slices");
    // The total limit and one piece past its slice, with room for a debug build.
    let took = started.elapsed();
    assert!(took < Duration::from_secs(5), "{took:?}");
    let highlights = document.highlights();
    assert_eq!(highlights.len(), text.len());
    assert!(
        highlights
            .runs(0..text.len())
            .all(|(_, highlight)| highlight.is_none())
    );
}

proptest! {
    #![proptest_config(cases(24))]

    /// Any grammar, any text: it highlights to the end, and the invariants hold (ADR 0111, Q12).
    #[test]
    fn random_text_highlights_on_a_small_stack(
        language in 0..LANGUAGES.len(),
        text in "(\\PC|\n){0,3000}",
    ) {
        let result = highlight_on_a_small_stack(LANGUAGES[language], text, SLICE);
        prop_assert!(result.is_ok(), "{:?}", result);
    }

    /// Any grammar, any few atoms, repeated past the inline cap.
    #[test]
    fn crafted_nesting_highlights_on_a_small_stack(
        language in 0..LANGUAGES.len(),
        atoms in prop::collection::vec(0..ATOMS.len(), 1..4),
    ) {
        let text = repeated(&atoms, BYTES);
        let result = highlight_on_a_small_stack(LANGUAGES[language], text, SLICE);
        prop_assert!(result.is_ok(), "{:?}", result);
    }
}
