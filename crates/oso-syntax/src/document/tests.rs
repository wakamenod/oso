//! Tests of documents: a parse by slices, edits that wait, the limits, and an incremental
//! parse that gives the tree a parse from nothing gives (ADR 0111).
//!
//! Must not depend on: the worker; these call a document directly.

use std::time::Duration;

use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use proptest::prelude::*;
use tree_sitter::Tree;

use super::{Document, Failure, MAX_BYTES, Parsed};
use crate::language::Language;

const LONG: Duration = Duration::from_secs(30);

/// A node's kind, its bytes, and its first and last points as rows and columns.
type Node = (String, usize, usize, (usize, usize), (usize, usize));

/// Every node of `tree`, in order, walked with a cursor.
fn nodes(tree: &Tree) -> Vec<Node> {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        let (start, end) = (node.start_position(), node.end_position());
        out.push((
            node.kind().to_owned(),
            node.start_byte(),
            node.end_byte(),
            (start.row, start.column),
            (end.row, end.column),
        ));
        if cursor.goto_first_child() || cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                return out;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn parsed(language: Language, text: &str) -> Document {
    let mut document = Document::new(language, Rope::from(text));
    assert_eq!(document.parse(LONG), Parsed::Done);
    document
}

/// Applies a batch to `text` as an editor does, highest first, and returns the text after.
fn apply(text: &Rope, batch: &[(Splice, String)]) -> Rope {
    let mut after = text.clone();
    for (splice, new) in batch.iter().rev() {
        after.replace(splice.start..splice.start + splice.old_len, new);
    }
    after
}

#[test]
fn each_language_parses_its_text() {
    let cases = [
        (
            Language::Rust,
            "fn main() {\n    let x = \"日本\";\n}\n",
            "source_file",
        ),
        (Language::Markdown, "# Title\n\nSome *text*.\n", "document"),
        (
            Language::MarkdownInline,
            "Some *text* and `code`.",
            "inline",
        ),
        (Language::Toml, "[package]\nname = \"oso\"\n", "document"),
    ];
    for (language, text, root) in cases {
        let document = parsed(language, text);
        let tree = document.tree().unwrap();
        assert_eq!(tree.root_node().kind(), root, "{}", language.name());
        assert!(!tree.root_node().has_error(), "{}", language.name());
    }
}

#[test]
fn a_slice_stops_a_parse_and_the_next_call_goes_on() {
    let text = "fn f(a: u32) -> u32 { a + 1 }\n".repeat(20_000);
    let mut document = Document::new(Language::Rust, Rope::from(text.as_str()));
    let mut calls = 0;
    while document.parse(Duration::from_micros(200)) == Parsed::Paused {
        calls += 1;
        assert!(calls < 100_000, "the parse never ends");
    }
    assert!(calls > 0, "a slice of 200 µs finished a 600 KB parse");
    let fresh = parsed(Language::Rust, &text);
    assert_eq!(
        nodes(document.tree().unwrap()),
        nodes(fresh.tree().unwrap())
    );
}

#[test]
fn edits_wait_for_a_stopped_parse_and_follow_it() {
    let text = Rope::from("fn f() {}\n".repeat(20_000).as_str());
    let mut document = Document::new(Language::Rust, text.clone());
    assert_eq!(document.parse(Duration::from_micros(100)), Parsed::Paused);
    let batch = [(
        Splice {
            start: 3,
            old_len: 1,
            new_len: 4,
        },
        "main".to_owned(),
    )];
    let after = apply(&text, &batch);
    document.edit(&[batch[0].0], &after);
    assert_eq!(document.parse(LONG), Parsed::Done);
    let fresh = parsed(Language::Rust, &after.to_string());
    assert_eq!(
        nodes(document.tree().unwrap()),
        nodes(fresh.tree().unwrap())
    );
}

#[test]
fn splices_that_do_not_fit_parse_again_from_nothing() {
    let mut document = parsed(Language::Toml, "a = 1\n");
    let after = Rope::from("[t]\nb = \"x\"\n");
    let wrong = Splice {
        start: 0,
        old_len: 1,
        new_len: 1,
    };
    document.edit(&[wrong], &after);
    assert_eq!(document.parse(LONG), Parsed::Done);
    let fresh = parsed(Language::Toml, "[t]\nb = \"x\"\n");
    assert_eq!(
        nodes(document.tree().unwrap()),
        nodes(fresh.tree().unwrap())
    );
}

#[test]
fn a_text_past_the_size_limit_is_not_parsed() {
    let text = "x".repeat(MAX_BYTES + 1);
    let mut document = Document::new(Language::Markdown, Rope::from(text.as_str()));
    assert_eq!(document.parse(LONG), Parsed::Failed(Failure::TooLarge));
    assert!(document.tree().is_none());
    document.edit(&[], &Rope::from("small"));
    assert_eq!(document.parse(LONG), Parsed::Failed(Failure::TooLarge));
}

#[test]
fn a_parse_past_its_time_limit_stops_for_good() {
    let text = "fn f(a: u32) -> u32 { a + 1 }\n".repeat(20_000);
    let mut document = Document::new(Language::Rust, Rope::from(text.as_str()));
    document.limits = (Duration::from_micros(300), Duration::from_micros(300));
    let mut result = Parsed::Paused;
    for _ in 0..100_000 {
        result = document.parse(Duration::from_micros(100));
        if result != Parsed::Paused {
            break;
        }
    }
    assert_eq!(result, Parsed::Failed(Failure::TooSlow));
    assert!(document.tree().is_none());
}

/// Pieces that edits insert: tokens of each language, line breaks, and text of several bytes
/// a character, so that points count bytes.
const PIECES: [&str; 16] = [
    "fn ", "{", "}", "\n", "\"", "//", " ", "x", "日本", "# ", "*", "`", "[a]", " = ", "1", "é",
];

fn batch(len: usize) -> impl Strategy<Value = Vec<(Splice, String)>> {
    prop::collection::vec(
        (
            0..=len,
            0usize..6,
            prop::collection::vec(0..PIECES.len(), 0..4),
        ),
        1..4,
    )
    .prop_map(move |mut picks| {
        picks.sort_by_key(|pick| pick.0);
        let mut out: Vec<(Splice, String)> = Vec::new();
        let mut end = 0;
        for (start, old_len, pieces) in picks {
            let start = start.max(end);
            let old_len = old_len.min(len.saturating_sub(start));
            if start > len {
                break;
            }
            let new: String = pieces.iter().map(|&i| PIECES[i]).collect();
            out.push((
                Splice {
                    start,
                    old_len,
                    new_len: new.len(),
                },
                new,
            ));
            end = start + old_len;
        }
        out
    })
}

/// Keeps offsets on character boundaries: a start or an end inside a character moves to its
/// start, as the editor's offsets never fall inside one.
fn on_boundaries(text: &str, batch: Vec<(Splice, String)>) -> Vec<(Splice, String)> {
    let floor = |mut at: usize| {
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let mut out: Vec<(Splice, String)> = Vec::new();
    let mut end = 0;
    for (splice, new) in batch {
        let start = floor(splice.start).max(end);
        let old_end = floor(splice.start + splice.old_len).max(start);
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

const SEEDS: [(Language, &str); 3] = [
    (
        Language::Rust,
        "fn main() {\n    let s = \"日本\"; // note\n}\n",
    ),
    (
        Language::Markdown,
        "# Title\n\n- item *one*\n\n```rust\nx\n```\n",
    ),
    (Language::Toml, "[package]\nname = \"oso\" # é\n[[bin]]\n"),
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn an_incremental_parse_gives_the_tree_of_a_fresh_one(
        seed in 0..SEEDS.len(),
        batches in prop::collection::vec(batch(64), 1..6),
    ) {
        let (language, text) = SEEDS[seed];
        let mut document = parsed(language, text);
        let mut text = Rope::from(text);
        for batch in batches {
            let batch = on_boundaries(&text.to_string(), batch);
            let batch: Vec<_> = batch
                .into_iter()
                .filter(|(splice, _)| splice.start + splice.old_len <= text.len())
                .collect();
            let after = apply(&text, &batch);
            let splices: Vec<Splice> = batch.iter().map(|(splice, _)| *splice).collect();
            document.edit(&splices, &after);
            prop_assert_eq!(document.parse(LONG), Parsed::Done);
            text = after;
        }
        let fresh = parsed(language, &text.to_string());
        prop_assert_eq!(nodes(document.tree().unwrap()), nodes(fresh.tree().unwrap()));
    }
}
