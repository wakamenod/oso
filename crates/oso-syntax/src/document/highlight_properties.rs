//! Properties of highlighting under random edits, on texts of several rope chunks (ADR 0111).
//!
//! Must not depend on: the worker; these call a document directly.
//!
//! - Highlighted after each batch, a document has the highlights a fresh one of its text has,
//!   wherever neither tree holds an error: tree-sitter does not promise more.
//! - Always, the highlights cover the text; once done, no line break is highlighted; and slices
//!   of any length end where one long slice does, where neither tree holds an error, or fail as
//!   too slow and stay plain.

use std::ops::Range;
use std::time::Duration;

use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Splice};
use proptest::prelude::*;

use super::highlight_tests::highlighted;
use super::properties::{Drawn, SEEDS, batches, splices_of};
use super::tests::{LONG, apply, cases};
use super::{Document, Failure, Highlighted};
use crate::queries::Queries;

type Runs = Vec<(Range<usize>, Option<Highlight>)>;

fn all_runs(document: &Document) -> Runs {
    let highlights = document.highlights();
    highlights.runs(0..highlights.len()).collect()
}

fn clean(document: &Document) -> bool {
    document
        .tree
        .as_ref()
        .is_some_and(|tree| !tree.root_node().has_error())
}

/// Applies `drawn` to `text` and the document, and returns the text after.
fn edit(document: &mut Document, text: &Rope, drawn: &[Drawn]) -> Rope {
    let batch = splices_of(&text.to_string(), drawn);
    let after = apply(text, &batch);
    let splices: Vec<Splice> = batch.iter().map(|(splice, _)| *splice).collect();
    document.edit(&splices, &after);
    after
}

fn line_breaks_plain(document: &Document, text: &Rope) -> Result<(), TestCaseError> {
    let text = text.to_string();
    for (at, _) in text.match_indices('\n') {
        prop_assert_eq!(document.highlights().at(at), None, "line break at {}", at);
    }
    Ok(())
}

proptest! {
    #![proptest_config(cases(32))]

    /// Highlighting again what edits and the parse changed gives what highlighting all does.
    #[test]
    fn incremental_highlights_match_fresh_ones_where_clean(
        seed in 0..SEEDS.len(),
        batches in batches(),
    ) {
        let (language, unit, times) = SEEDS[seed];
        let mut text = Rope::from(unit.repeat(times).as_str());
        let mut document = highlighted(language, &text.to_string());
        let mut queries = Queries::new();
        for drawn in batches {
            text = edit(&mut document, &text, &drawn);
            prop_assert_eq!(document.highlight(&mut queries, &[], LONG), Highlighted::Done);
            let fresh = highlighted(language, &text.to_string());
            if clean(&document) && clean(&fresh) {
                prop_assert_eq!(all_runs(&document), all_runs(&fresh));
            }
        }
    }

    /// Edits between slices of any length keep the highlights over the text, and end in what
    /// one long slice gives.
    #[test]
    fn slices_resume_and_keep_the_invariants(
        seed in 0..SEEDS.len(),
        batches in batches(),
        slices in prop::collection::vec(0u64..400, 6),
    ) {
        let (language, unit, times) = SEEDS[seed];
        let mut text = Rope::from(unit.repeat(times).as_str());
        let mut sliced = Document::new(language, text.clone());
        let mut whole = Document::new(language, text.clone());
        let mut queries = Queries::new();
        for (drawn, micros) in batches.iter().zip(slices) {
            let paused = sliced.highlight(&mut queries, &[], Duration::from_micros(micros));
            // A slice too short for the least piece fails it as too slow, and it stays plain.
            let fine = matches!(
                paused,
                Highlighted::Paused | Highlighted::Done | Highlighted::Failed(Failure::TooSlow)
            );
            prop_assert!(fine, "{paused:?}");
            prop_assert_eq!(sliced.highlights().len(), text.len());
            prop_assert_eq!(whole.highlight(&mut queries, &[], LONG), Highlighted::Done);
            let after = edit(&mut sliced, &text, drawn);
            edit(&mut whole, &text, drawn);
            text = after;
            prop_assert_eq!(sliced.highlights().len(), text.len());
        }
        prop_assert_eq!(whole.highlight(&mut queries, &[], LONG), Highlighted::Done);
        let done = sliced.highlight(&mut queries, &[], LONG);
        line_breaks_plain(&sliced, &text)?;
        if done == Highlighted::Done {
            // Edits that wait for a stopped parse join, so the two parse different histories,
            // and tree-sitter promises the same tree only where neither holds an error.
            if clean(&sliced) && clean(&whole) {
                prop_assert_eq!(all_runs(&sliced), all_runs(&whole));
            }
        } else {
            prop_assert_eq!(done, Highlighted::Failed(Failure::TooSlow));
            prop_assert!(all_runs(&sliced).iter().all(|(_, highlight)| highlight.is_none()));
        }
    }
}
