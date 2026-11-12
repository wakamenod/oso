//! Properties of documents under random edits, on texts of several rope chunks (ADR 0111).
//!
//! Must not depend on: the worker; these call a document directly.
//!
//! tree-sitter does not promise that an incremental parse gives the tree a fresh one gives once
//! error recovery is involved. So a document is compared with the parse of the same history fed
//! one contiguous string, which must match it always, and with a fresh parse only where neither
//! tree holds an error.

use std::time::Duration;

use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use proptest::prelude::*;
use tree_sitter::{Parser, Tree};

use super::tests::{LONG, apply, cases, nodes, parsed};
use super::{Document, Parsed};
use crate::edit::input_edits;
use crate::language::Language;

/// Pieces that edits insert: tokens of each language, line breaks, text of several bytes a
/// character, so that points count bytes, a capital, which makes a Rust name a type with the
/// same tree, and a long one, so that an edit spans chunks.
const PIECES: [&str; 18] = [
    "fn ",
    "{",
    "}",
    "\n",
    "\"",
    "//",
    " ",
    "x",
    "Q",
    "日本",
    "# ",
    "*",
    "`",
    "[a]",
    " = ",
    "1",
    "é",
    "let long_name = \"a string of some length to cross a chunk of the rope\"; // 日本語の文\n",
];

/// One edit as drawn: where it starts, as a fraction of the text in ten-thousandths, how many
/// bytes it replaces, and the pieces of its new text.
pub(super) type Drawn = (u32, usize, Vec<usize>);

/// Texts of a few kilobytes, so that the rope's chunks of 1008 bytes split them.
pub(super) const SEEDS: [(Language, &str, usize); 3] = [
    (
        Language::Rust,
        "fn main() {\n    let s = \"日本\"; // note\n    if s.is_empty() { return; }\n}\n",
        40,
    ),
    (
        Language::Markdown,
        "# Title\n\n- item *one* and `code`\n\n```rust\nx\n```\n\n> quote\n",
        50,
    ),
    (
        Language::Toml,
        "[package]\nname = \"oso\" # é\n[[bin]]\nlist = [1, 2]\n",
        60,
    ),
];

/// A batch drawn as fractions, made into splices of `text`: ascending, apart, and on character
/// boundaries, as the editor's are.
pub(super) fn splices_of(text: &str, drawn: &[Drawn]) -> Vec<(Splice, String)> {
    let floor = |mut at: usize| {
        at = at.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let mut starts: Vec<(usize, usize, String)> = drawn
        .iter()
        .map(|(at, old_len, pieces)| {
            let start = floor(text.len() * *at as usize / 10_000);
            let new = pieces.iter().map(|&i| PIECES[i]).collect();
            (start, *old_len, new)
        })
        .collect();
    starts.sort_by_key(|(start, ..)| *start);
    let mut out = Vec::new();
    let mut end = 0;
    for (start, old_len, new) in starts {
        let start = start.max(end);
        let old_end = floor(start + old_len).max(start);
        out.push((
            Splice {
                start,
                old_len: old_end - start,
                new_len: new.len(),
            },
            new,
        ));
        end = old_end;
    }
    out
}

pub(super) fn batches() -> impl Strategy<Value = Vec<Vec<Drawn>>> {
    let edit = (
        0u32..=10_000,
        0usize..80,
        prop::collection::vec(0..PIECES.len(), 0..6),
    );
    prop::collection::vec(prop::collection::vec(edit, 1..5), 1..6)
}

/// The same history as a document's, parsed from one contiguous string.
struct Contiguous {
    parser: Parser,
    tree: Tree,
}

impl Contiguous {
    fn new(language: Language, text: &str) -> Self {
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(text, None).unwrap();
        Self { parser, tree }
    }

    fn follow(&mut self, before: &Rope, after: &Rope, splices: &[Splice]) {
        for edit in input_edits(before, after, splices).unwrap() {
            self.tree.edit(&edit);
        }
        self.tree = self
            .parser
            .parse(after.to_string(), Some(&self.tree))
            .unwrap();
    }
}

/// Compares a document's tree with a fresh parse of its text, where neither holds an error.
fn matches_fresh_where_clean(
    language: Language,
    tree: &Tree,
    text: &Rope,
) -> Result<(), TestCaseError> {
    let fresh = parsed(language, &text.to_string());
    let fresh = fresh.tree().unwrap();
    if !tree.root_node().has_error() && !fresh.root_node().has_error() {
        prop_assert_eq!(nodes(tree), nodes(fresh));
    }
    Ok(())
}

proptest! {
    #![proptest_config(cases(48))]

    /// Parsed after each batch, a document has the tree of the same history fed one string.
    #[test]
    fn a_document_parses_as_its_history_does(
        seed in 0..SEEDS.len(),
        batches in batches(),
    ) {
        let (language, unit, times) = SEEDS[seed];
        let mut text = Rope::from(unit.repeat(times).as_str());
        let mut document = parsed(language, &text.to_string());
        let mut contiguous = Contiguous::new(language, &text.to_string());
        for drawn in batches {
            let batch = splices_of(&text.to_string(), &drawn);
            let after = apply(&text, &batch);
            let splices: Vec<Splice> = batch.iter().map(|(splice, _)| *splice).collect();
            document.edit(&splices, &after);
            contiguous.follow(&text, &after, &splices);
            prop_assert_eq!(document.parse(LONG), Parsed::Done);
            prop_assert_eq!(nodes(document.tree().unwrap()), nodes(&contiguous.tree));
            text = after;
        }
        matches_fresh_where_clean(language, document.tree().unwrap(), &text)?;
    }

    /// Edits that come while a parse is stopped part way wait, and the tree it ends with is for
    /// the newest text.
    #[test]
    fn edits_during_a_stopped_parse_end_in_the_newest_text(
        seed in 0..SEEDS.len(),
        batches in batches(),
        stops in prop::collection::vec(any::<bool>(), 6),
    ) {
        let (language, unit, times) = SEEDS[seed];
        let mut text = Rope::from(unit.repeat(times).as_str());
        let mut document = Document::new(language, text.clone());
        for (drawn, stop) in batches.iter().zip(stops) {
            if stop {
                // 5 µs stops a parse of these texts part way (50 times in 50 when measured);
                // finishing early changes nothing here.
                let stopped = document.parse(Duration::from_micros(5));
                prop_assert!(matches!(stopped, Parsed::Paused | Parsed::Done), "{stopped:?}");
            }
            let batch = splices_of(&text.to_string(), drawn);
            let after = apply(&text, &batch);
            let splices: Vec<Splice> = batch.iter().map(|(splice, _)| *splice).collect();
            document.edit(&splices, &after);
            text = after;
        }
        prop_assert_eq!(document.parse(LONG), Parsed::Done);
        let tree = document.tree().unwrap();
        prop_assert_eq!(tree.root_node().end_byte(), text.len());
        matches_fresh_where_clean(language, tree, &text)?;
    }
}
