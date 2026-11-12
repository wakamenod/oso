//! Tests of matching a component without asking where it matched, which searches its literal
//! and its expression in one where it can, against matching that says where (ADR 0076), and a
//! timing of matching the names the benchmark's `C-x C-f` lists (ADR 0096).
//!
//! Must not depend on: `editor` or `minibuffer`; these run on strings alone.

use std::sync::Arc;

use proptest::prelude::*;

use super::Query;
use crate::complete::{Candidate, Candidates};

fn candidate(text: &str) -> Candidate {
    Candidate::new(text, "", "")
}

#[test]
fn a_small_expression_is_searched_with_its_literal_in_one() {
    // What a person types into a list of 100,000 files, case ignored or not.
    for input in ["e.", "e.*9", "Ä.*r", "ファ.*9", "txt$", "a|b"] {
        assert_eq!(Query::parse(input).joined(), [true], "{input:?}");
    }
    // An expression with a class, or too large to join, is searched in turn with its literal,
    // and matches as both do.
    let long = ".".repeat(400);
    let cases = [
        (r"\w{10}", "éééééééééé", true),
        (r"\w{10}", "ファイルファイルファイル", true),
        (r"\w{10}", r"x\w{10}x", true),
        (r"\w{10}", "short", false),
        ("[ab]c", "bc", true),
        ("[ab]c", "[ab]c", true),
        (&long, &"é".repeat(400), true),
        (&long, "short", false),
    ];
    for (input, text, matches) in cases {
        let query = Query::parse(input);
        assert_eq!(query.joined(), [false], "{input:?}");
        let item = candidate(text);
        assert_eq!(query.matches(&item), matches, "{input:?} in {text:?}");
        assert_eq!(
            query.matches_finding_where(&item),
            matches,
            "{input:?} in {text:?}"
        );
    }
    // A literal alone, and an expression that does not compile, have nothing to join.
    assert_eq!(Query::parse("e (").joined(), [false, false]);
}

proptest! {
    #[test]
    fn matching_agrees_with_finding_where(
        texts in prop::collection::vec("[a-cA-CéİK.^$()|*+?\\\\ -]{0,8}", 1..8),
        input in "[a-cA-CéİK !,~^=&.^$()|*+?{}\\[\\]\\\\-]{0,6}",
    ) {
        let query = Query::parse(&input);
        for text in &texts {
            let item = Candidate::new(text.as_str(), "", text.as_str());
            prop_assert_eq!(
                query.matches(&item),
                query.matches_finding_where(&item),
                "{:?} in {:?}",
                input,
                text
            );
        }
    }
}

/// What a worker takes to match each key the benchmark types into `C-x C-f` over 100,000 names
/// (`examples/perf/bench/finding.rs`, ADR 0096). Run it by hand in release, and read what it
/// prints: `cargo test --release -p oso-core matching_100000 -- --ignored --nocapture`.
#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(
    clippy::disallowed_macros,
    reason = "the timing is read from what it prints"
)]
fn matching_100000_file_names_a_key() {
    use std::time::Instant;
    let stems = ["file", "ファイル", "résumé", "Ärger"];
    let items: Arc<[Candidate]> = (0..100_000)
        .map(|n| candidate(&format!("{}-{n:05}.txt", stems[n % stems.len()])))
        .collect();
    let mut candidates = Candidates::new(items, []);
    let inputs = ["e", "e.", "e.*", "e.*9", "e.*", "e.", "e", ""];
    // The first round warms the caches; the second is read.
    for round in 0..2 {
        for input in inputs {
            let start = Instant::now();
            candidates.set_input(input);
            if round == 1 {
                eprintln!(
                    "{input:5} {:>9.2?} {} matches",
                    start.elapsed(),
                    candidates.len()
                );
            }
        }
    }
}
