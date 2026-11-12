//! Tests of highlighting: each language's captures, painting, line breaks, pieces, Markdown's
//! inline text and its cap (ADR 0111).
//!
//! Must not depend on: the worker; these call a document directly.

use oso_core::rope::Rope;
use oso_core::syntax::Highlight::{
    self, Comment, Constant, Function, Heading, Keyword, Link, Macro, Markup, Property, String,
    Type,
};
use oso_core::syntax::Splice;

use super::inline::MAX_INLINE_BYTES;
use super::tests::LONG;
use super::{Document, Highlighted};
use crate::language::Language;
use crate::queries::Queries;

/// A document of `text`, highlighted to the end.
pub(super) fn highlighted(language: Language, text: &str) -> Document {
    let mut document = Document::new(language, Rope::from(text));
    assert_eq!(
        document.highlight(&mut Queries::new(), &[], LONG),
        Highlighted::Done
    );
    document
}

/// Each highlighted run's text, with what it is.
pub(super) fn runs(document: &Document) -> Vec<(std::string::String, Highlight)> {
    let text = document.text.to_string();
    let highlights = document.highlights();
    highlights
        .runs(0..highlights.len())
        .filter_map(|(range, highlight)| Some((text[range].to_owned(), highlight?)))
        .collect()
}

fn has(runs: &[(std::string::String, Highlight)], text: &str, highlight: Highlight) -> bool {
    runs.iter().any(|(run, h)| run == text && *h == highlight)
}

#[track_caller]
fn assert_has(language: Language, text: &str, expected: &[(&str, Highlight)]) {
    let runs = runs(&highlighted(language, text));
    for (part, highlight) in expected {
        assert!(
            has(&runs, part, *highlight),
            "{part:?} as {highlight:?} in {runs:?}"
        );
    }
}

#[test]
fn rust_captures() {
    let text = "#[derive(Debug)]\nstruct Point<'a> { x: &'a str }\n\
                // note\nfn main() {\n    let MAX = 1;\n    println!(\"hi\\n\");\n    \
                foo(Some(true), 2.5);\n    point.x.len();\n}\n";
    assert_has(
        Language::Rust,
        text,
        &[
            ("#[derive(", Macro),
            ("Debug", Type),
            ("struct", Keyword),
            ("Point", Type),
            ("'a", Keyword),
            ("x", Property),
            ("str", Type),
            ("// note", Comment),
            ("fn", Keyword),
            ("main", Function),
            ("MAX", Constant),
            ("println!", Macro),
            ("\"hi\\n\"", String),
            ("foo", Function),
            ("Some", Type),
            ("true", Constant),
            ("2.5", Constant),
            ("len", Function),
        ],
    );
}

#[test]
fn of_two_rust_patterns_on_one_name_the_first_wins() {
    // `ALL_CAPS` matches both the constant and the constructor convention.
    let runs = runs(&highlighted(Language::Rust, "fn f() { g(ALL_CAPS); }\n"));
    assert!(has(&runs, "ALL_CAPS", Constant), "{runs:?}");
}

#[test]
fn markdown_captures() {
    let text = "# Title *here*\n\n- item with `code` and **bold**\n\n\
                > a quote\n> [link](https://example.com)\n\n| a | *b* |\n|---|---|\n";
    assert_has(
        Language::Markdown,
        text,
        &[
            ("#", Markup),
            ("Title ", Heading),
            ("*here*", Markup),
            ("- ", Markup),
            ("`code`", String),
            ("**bold**", Markup),
            ("> ", Markup),
            ("link", Link),
            ("https://example.com", Link),
            ("*b*", Markup),
        ],
    );
}

#[test]
fn markdown_code_stays_plain() {
    let document = highlighted(Language::Markdown, "```rust\nfn main() {}\n```\n");
    let start = "```rust\n".len();
    let plain = document
        .highlights()
        .runs(start..start + 12)
        .all(|(_, h)| h.is_none());
    assert!(plain, "{:?}", runs(&document));
}

#[test]
fn toml_captures() {
    let text = "# top\n[package]\nname = \"oso\"\nversion.major = 1\n\
                ok = true\nwhen = 1979-05-27\n[[bin]]\n";
    assert_has(
        Language::Toml,
        text,
        &[
            ("# top", Comment),
            ("package", Type),
            ("name", Property),
            ("\"oso\"", String),
            ("version.major", Property),
            ("1", Constant),
            ("true", Constant),
            ("1979-05-27", String),
            ("bin", Type),
        ],
    );
}

#[test]
fn line_breaks_are_never_highlighted() {
    let cases = [
        (
            Language::Rust,
            "/* one\ntwo\n\nthree */\nlet s = \"a\nb\";\n",
        ),
        (Language::Markdown, "Setext *a\nb*\n===\n\n> q\n> r\n"),
        (Language::Toml, "s = \"\"\"\nmulti\n\"\"\"\n"),
    ];
    for (language, text) in cases {
        let document = highlighted(language, text);
        let highlights = document.highlights();
        for (at, _) in text.match_indices('\n') {
            assert_eq!(highlights.at(at), None, "{language:?} at {at}");
        }
        assert!(!runs(&document).is_empty(), "{language:?}");
    }
}

#[test]
fn highlights_follow_edits_and_the_edited_lines_are_highlighted_again() {
    let mut document = highlighted(Language::Rust, "fn f() {}\nfn g() {}\n");
    // `fn g` becomes `fn G`; the guess keeps it a function until highlighting again.
    let after = Rope::from("fn f() {}\nlet G = 1;\n");
    document.edit(
        &[Splice {
            start: 10,
            old_len: 9,
            new_len: 10,
        }],
        &after,
    );
    assert_eq!(document.highlights().len(), after.len());
    assert!(!document.pending.ranges().is_empty());
    let done = document.highlight(&mut Queries::new(), &[], LONG);
    assert_eq!(done, Highlighted::Done);
    let runs = runs(&document);
    assert!(has(&runs, "let", Keyword), "{runs:?}");
    assert!(has(&runs, "1", Constant), "{runs:?}");
    assert!(has(&runs, "f", Function), "{runs:?}");
}

#[test]
fn shown_text_goes_first() {
    let line = "fn f() {}\n";
    let text = line.repeat(30_000);
    let mut document = Document::new(Language::Rust, Rope::from(text.as_str()));
    assert_eq!(document.parse(LONG), super::Parsed::Done);
    let shown = text.len() - 100..text.len() - 50;
    let piece = document.next_piece(std::slice::from_ref(&shown)).unwrap();
    assert!(
        piece.start <= shown.start && shown.start < piece.end,
        "{piece:?}"
    );
    assert_eq!(piece.start % line.len(), 0, "a piece starts a line");
    let rest = document.next_piece(&[]).unwrap();
    assert_eq!(rest.start, 0);
    assert!(rest.end - rest.start <= super::highlight::PIECE);
    assert_eq!(rest.end % line.len(), 0, "a piece ends a line");
}

#[test]
fn a_long_line_is_cut_at_a_character() {
    let text = format!("// {}\n", "日本".repeat(100_000));
    let mut document = Document::new(Language::Rust, Rope::from(text.as_str()));
    assert_eq!(document.parse(LONG), super::Parsed::Done);
    let piece = document.next_piece(&[]).unwrap();
    assert!(piece.end - piece.start <= super::highlight::PIECE);
    assert!(text.is_char_boundary(piece.end));
    let done = document.highlight(&mut Queries::new(), &[], LONG);
    assert_eq!(done, Highlighted::Done);
    let highlights = document.highlights();
    assert_eq!(highlights.at(text.len() - 2), Some(Comment));
    assert_eq!(highlights.at(text.len() - 1), None);
}

#[test]
fn an_inline_node_past_the_cap_stays_plain() {
    let unit = "*a* ";
    let under = unit.repeat(MAX_INLINE_BYTES / unit.len() - 1);
    let over = unit.repeat(MAX_INLINE_BYTES / unit.len() + 1);
    let runs_under = runs(&highlighted(Language::Markdown, &format!("{under}\n")));
    assert!(has(&runs_under, "*a*", Markup));
    let document = highlighted(Language::Markdown, &format!("{over}\n\n*b*\n"));
    let runs_over = runs(&document);
    assert!(!has(&runs_over, "*a*", Markup), "past the cap");
    assert!(has(&runs_over, "*b*", Markup), "the next paragraph");
}

#[test]
fn a_slice_resumes_where_it_stopped() {
    let text = "fn f() { let x = \"s\"; } // c\n".repeat(20_000);
    let whole = runs(&highlighted(Language::Rust, &text));
    let mut document = Document::new(Language::Rust, Rope::from(text.as_str()));
    // Slices this short judge no piece too slow, and a slow runner meets no total limit: the
    // test holds whatever the machine's speed.
    document.limits.2 = LONG;
    let mut queries = Queries::new();
    let mut calls = 0;
    loop {
        match document.highlight(&mut queries, &[], std::time::Duration::from_millis(5)) {
            Highlighted::Done => break,
            Highlighted::Paused => calls += 1,
            failed => panic!("{failed:?} after {calls} slices"),
        }
    }
    assert!(calls > 1, "the slices stopped it");
    assert_eq!(runs(&document), whole);
}

#[test]
fn a_failed_document_is_plain() {
    let mut document = highlighted(Language::Rust, "fn f() {}\n");
    let after = Rope::from("x".repeat(super::MAX_BYTES + 1).as_str());
    document.edit(
        &[Splice {
            start: 0,
            old_len: 10,
            new_len: after.len(),
        }],
        &after,
    );
    let failed = document.highlight(&mut Queries::new(), &[], LONG);
    assert_eq!(failed, Highlighted::Failed(super::Failure::TooLarge));
    assert!(runs(&document).is_empty());
    assert_eq!(document.highlights().len(), after.len());
}

#[test]
fn a_name_too_long_for_a_predicate_matches_none() {
    let long = format!("X{}", "y".repeat(2 << 10));
    let text = format!("fn f() {{ g(Short, {long}); }}\n");
    let runs = runs(&highlighted(Language::Rust, &text));
    assert!(has(&runs, "Short", Type), "{runs:?}");
    assert!(
        !runs.iter().any(|(run, _)| run == &long),
        "the long name is plain"
    );
}
