//! Tests of documents: a parse by slices, edits that wait, the limits, and an incremental
//! parse that gives the tree a parse from nothing gives (ADR 0111).
//!
//! Must not depend on: the worker; these call a document directly.

use std::time::Duration;

use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use tree_sitter::Tree;

use super::{Document, Failure, MAX_BYTES, Parsed};
use crate::language::Language;

pub(super) const LONG: Duration = Duration::from_secs(30);

/// A property test's number of cases: `default`, or `PROPTEST_CASES` if it is set, so that the
/// weekly workflow runs many more.
pub(super) fn cases(default: u32) -> proptest::prelude::ProptestConfig {
    let config = proptest::prelude::ProptestConfig::default();
    if std::env::var_os("PROPTEST_CASES").is_some() {
        config
    } else {
        proptest::prelude::ProptestConfig {
            cases: default,
            ..config
        }
    }
}

/// A node's kind, its bytes, and its first and last points as rows and columns.
pub(super) type Node = (String, usize, usize, (usize, usize), (usize, usize));

/// Every node of `tree`, in order, walked with a cursor.
pub(super) fn nodes(tree: &Tree) -> Vec<Node> {
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

pub(super) fn parsed(language: Language, text: &str) -> Document {
    let mut document = Document::with_long_lines(language, Rope::from(text));
    assert_eq!(document.parse(LONG), Parsed::Done);
    document
}

/// Applies a batch to `text` as an editor does, highest first, and returns the text after.
pub(super) fn apply(text: &Rope, batch: &[(Splice, String)]) -> Rope {
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
        (
            Language::Go,
            "package main\n\nfunc main() {\n\ts := \"日本\"\n}\n",
            "source_file",
        ),
        (Language::JavaScript, "const a = <p>日本</p>;\n", "program"),
        (
            Language::TypeScript,
            "const a: string = <string>b;\n",
            "program",
        ),
        (Language::Tsx, "const a: string = <p>日本</p>;\n", "program"),
        (
            Language::Html,
            "<!DOCTYPE html>\n<p class=\"a\">日本 &amp;</p>\n<script>f()</script>\n",
            "document",
        ),
        (
            Language::Css,
            "a.b > #c:hover { color: red; }\n",
            "stylesheet",
        ),
        (Language::Json, "{\"a\": [1, true, \"日本\"]}\n", "document"),
        (Language::Yaml, "a: 日本\nb:\n  - 1\n", "stream"),
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
    let mut document = Document::with_long_lines(Language::Rust, Rope::from(text.as_str()));
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
    let mut document = Document::with_long_lines(Language::Rust, text.clone());
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
    let mut document = Document::with_long_lines(Language::Markdown, Rope::from(text.as_str()));
    assert_eq!(document.parse(LONG), Parsed::Failed(Failure::TooLarge));
    assert!(document.tree().is_none());
    document.edit(&[], &Rope::from("small"));
    assert_eq!(document.parse(LONG), Parsed::Failed(Failure::TooLarge));
}

#[test]
fn a_parse_past_its_time_limit_stops_for_good() {
    let text = "fn f(a: u32) -> u32 { a + 1 }\n".repeat(20_000);
    let mut document = Document::with_long_lines(Language::Rust, Rope::from(text.as_str()));
    document.limits = (
        Duration::from_micros(300),
        Duration::from_micros(300),
        Duration::from_micros(300),
    );
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

#[test]
fn edits_past_what_may_wait_parse_again_from_nothing() {
    let text = Rope::from("ab\n".repeat(20_000).as_str());
    let mut document = Document::with_long_lines(Language::Rust, text.clone());
    // A parse stopped part way: the first call may spend its slice reading the lines.
    while document.running.is_none() {
        assert_eq!(document.parse(Duration::from_micros(5)), Parsed::Paused);
    }
    // One batch of more splices than may wait, one each line.
    let batch: Vec<(Splice, String)> = (0..super::MAX_WAITING + 1)
        .map(|line| {
            (
                Splice {
                    start: line * 3,
                    old_len: 1,
                    new_len: 1,
                },
                "c".to_owned(),
            )
        })
        .collect();
    let after = apply(&text, &batch);
    let splices: Vec<Splice> = batch.iter().map(|(splice, _)| *splice).collect();
    document.edit(&splices, &after);
    assert!(
        document
            .waiting
            .as_ref()
            .is_some_and(|(batch, _)| batch.is_none()),
        "too many to wait"
    );
    assert_eq!(document.parse(LONG), Parsed::Done);
    let fresh = parsed(Language::Rust, &after.to_string());
    assert_eq!(
        nodes(document.tree().unwrap()),
        nodes(fresh.tree().unwrap())
    );
}

/// A text with a line longer than 16 KiB is not parsed, and stays plain; so is one an edit
/// gives such a line, where it joins two (ADR 0147).
#[test]
fn a_line_past_the_limit_stops_the_document() {
    let long = "x".repeat(super::long_line::MAX_LINE_BYTES + 1);
    let mut document = Document::new(
        Language::Go,
        Rope::from(format!("var a\nvar b = \"{long}\"\n").as_str()),
    );
    assert_eq!(document.parse(LONG), Parsed::Failed(Failure::LongLine));

    let half = "x".repeat(super::long_line::MAX_LINE_BYTES / 2 + 1);
    let before = format!("{half}\n{half}\n");
    let mut document = Document::new(Language::Go, Rope::from(before.as_str()));
    assert_eq!(document.parse(LONG), Parsed::Done);
    let after = Rope::from(format!("{half}{half}\n").as_str());
    let join = Splice {
        start: half.len(),
        old_len: 1,
        new_len: 0,
    };
    document.edit(&[join], &after);
    assert_eq!(document.parse(LONG), Parsed::Failed(Failure::LongLine));
    assert_eq!(document.highlights().len(), after.len());
    assert!(
        document
            .highlights()
            .runs(0..after.len())
            .all(|(_, h)| h.is_none())
    );
}
