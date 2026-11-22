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
use super::{Document, Highlighted, wall};
use crate::language::Language;
use crate::queries::Queries;

/// A document of `text`, highlighted to the end.
pub(super) fn highlighted(language: Language, text: &str) -> Document {
    let mut document = Document::with_long_lines(language, Rope::from(text));
    assert_eq!(
        document.highlight(&mut Queries::new(), &[], LONG, wall),
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
fn go_captures() {
    let text = "package main\n\nimport \"fmt\"\n\nconst Limit = 10\n\n\
                type Point[T any] struct {\n\tX, Y T\n}\n\n\
                // Below is below.\nfunc (p *Point[T]) Below(n int) bool {\n\
                \tq := Point[int]{X: 1}\n\tfmt.Println(`raw`, 'r', 2.5i, nil, iota, true)\n\
                \tgo p.run[int]()\nloop:\n\tfor range make(chan int) {\n\t\tbreak loop\n\t}\n\
                \treturn len(q.X) > n\n}\n";
    assert_has(
        Language::Go,
        text,
        &[
            ("package", Keyword),
            ("\"fmt\"", String),
            ("const", Keyword),
            ("Limit", Constant),
            ("10", Constant),
            ("Point", Type),
            ("any", Type),
            ("struct", Keyword),
            ("X", Property),
            ("// Below is below.", Comment),
            ("Below", Function),
            ("int", Type),
            ("Println", Function),
            ("`raw`", String),
            ("'r'", String),
            ("2.5i", Constant),
            ("nil", Constant),
            ("iota", Constant),
            ("true", Constant),
            ("go", Keyword),
            ("run", Function),
            ("loop", Keyword),
            ("make", Function),
            ("chan", Keyword),
            ("len", Function),
        ],
    );
    // A package's name, like a variable, is plain.
    let runs = runs(&highlighted(Language::Go, text));
    assert!(
        !runs.iter().any(|(run, _)| run == "fmt" || run == "q"),
        "{runs:?}"
    );
}

const ECMA: &str = "#!/usr/bin/env node\nimport { readFile } from \"fs\";\n\
                    const MAX_SIZE = 10, Answer = 42;\n// note\n\
                    export async function load(path, opts = {}) {\n\
                    \tconst text = await readFile(path, 'utf8');\n\
                    \tconst re = /a+b/g;\n\tthis.size = text.length;\n\
                    \treturn `size ${text.trim()} of ${path}`;\n}\n\
                    class Shelf extends Base {\n\t#items = [];\n\tget count() { return null; }\n\
                    \tadd(x) { this.#items.push(x); super.add(undefined, true, 1.5); }\n}\n\
                    const square = (n) => n * n;\n";

/// What JavaScript, TypeScript and TSX share, as each draws it.
#[track_caller]
fn assert_ecma(language: Language) {
    assert_has(
        language,
        ECMA,
        &[
            ("#!/usr/bin/env node", Comment),
            ("import", Keyword),
            ("\"fs\"", String),
            ("MAX_SIZE", Constant),
            ("Answer", Type),
            ("// note", Comment),
            ("async", Keyword),
            ("load", Function),
            ("await", Keyword),
            ("readFile", Function),
            ("'utf8'", String),
            ("/a+b/g", String),
            ("this", Keyword),
            ("size", Property),
            ("length", Property),
            ("`size ", String),
            ("trim", Function),
            (" of ", String),
            ("`", String),
            ("class", Keyword),
            ("Shelf", Type),
            ("#items", Property),
            ("get", Keyword),
            ("count", Function),
            ("null", Constant),
            ("push", Function),
            ("super", Keyword),
            ("undefined", Constant),
            ("true", Constant),
            ("1.5", Constant),
            ("square", Function),
        ],
    );
    // A template's code is code: its names stay plain.
    let runs = runs(&highlighted(language, ECMA));
    assert!(
        !runs.iter().any(|(run, _)| run == "text" || run == "path"),
        "{runs:?}"
    );
}

#[test]
fn javascript_captures() {
    assert_ecma(Language::JavaScript);
    let jsx = "const App = () => (\n\t<Menu.Item onClick={go} label=\"x\">\n\
               \t\t<div className='a'>Hello {name}</div>\n\t\t<Button />\n\t</Menu.Item>\n);\n";
    assert_has(
        Language::JavaScript,
        jsx,
        &[
            ("App", Function),
            ("Menu", Function),
            ("Item", Function),
            ("onClick", Property),
            ("\"x\"", String),
            ("div", Function),
            ("className", Property),
            ("Button", Function),
        ],
    );
    let runs = runs(&highlighted(Language::JavaScript, jsx));
    assert!(
        !runs.iter().any(|(run, _)| run.contains("Hello")),
        "{runs:?}"
    );
}

const TYPESCRIPT: &str = "interface Point<T> { x: T; area(): number; }\n\
                          type Id = string | undefined;\n\
                          enum Color { Red, Green = 2 }\n\
                          declare function f(a: Point<number>): a is Point<number>;\n\
                          abstract class Shape implements Drawn {\n\
                          \tprivate readonly name: string = 'shape';\n\
                          \t@Input() size?: number;\n\tabstract draw(): void;\n}\n\
                          const n = f(p) satisfies boolean;\n";

#[test]
fn typescript_captures() {
    assert_ecma(Language::TypeScript);
    assert_has(
        Language::TypeScript,
        TYPESCRIPT,
        &[
            ("interface", Keyword),
            ("Point", Type),
            ("T", Type),
            ("x", Property),
            ("area", Function),
            ("number", Type),
            ("type", Keyword),
            ("Id", Type),
            ("string", Type),
            ("enum", Keyword),
            ("Red", Constant),
            ("Green", Constant),
            ("declare", Keyword),
            ("f", Function),
            ("is", Keyword),
            ("abstract", Keyword),
            ("Shape", Type),
            ("implements", Keyword),
            ("private", Keyword),
            ("readonly", Keyword),
            ("'shape'", String),
            ("@Input", Macro),
            ("draw", Function),
            ("void", Type),
            ("satisfies", Keyword),
            ("boolean", Type),
        ],
    );
    // `<T>value` is a cast in TypeScript; TSX has no such cast.
    assert_has(
        Language::TypeScript,
        "const v = <Point>p;\n",
        &[("Point", Type)],
    );
}

#[test]
fn tsx_captures() {
    assert_ecma(Language::Tsx);
    assert_has(
        Language::Tsx,
        TYPESCRIPT,
        &[("interface", Keyword), ("Red", Constant)],
    );
    let tsx = "export function Card({ title }: Props): JSX.Element {\n\
               \treturn <section aria-label={title}><Header<string> text=\"t\" /></section>;\n}\n";
    assert_has(
        Language::Tsx,
        tsx,
        &[
            ("Card", Function),
            ("Props", Type),
            ("section", Function),
            ("Header", Function),
            ("string", Type),
            ("text", Property),
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
        (Language::Go, "var s = `raw\nlines`\n/* a\nb */\n"),
        (Language::JavaScript, "const s = `a\nb`;\n/* a\nb */\n"),
        (Language::TypeScript, "let s: string = `a\n${x}\nb`;\n"),
        (Language::Tsx, "const e = <div>\n\ttext\n</div>;\n"),
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
    let done = document.highlight(&mut Queries::new(), &[], LONG, wall);
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
    let mut document = Document::with_long_lines(Language::Rust, Rope::from(text.as_str()));
    assert_eq!(document.parse(LONG, wall), super::Parsed::Done);
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
    let mut document = Document::with_long_lines(Language::Rust, Rope::from(text.as_str()));
    assert_eq!(document.parse(LONG, wall), super::Parsed::Done);
    let piece = document.next_piece(&[]).unwrap();
    assert!(piece.end - piece.start <= super::highlight::PIECE);
    assert!(text.is_char_boundary(piece.end));
    let done = document.highlight(&mut Queries::new(), &[], LONG, wall);
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
    let mut document = Document::with_long_lines(Language::Rust, Rope::from(text.as_str()));
    // Slices this short judge no piece too slow, and a slow runner meets no total limit: the
    // test holds whatever the machine's speed.
    document.limits.2 = LONG;
    let mut queries = Queries::new();
    let mut calls = 0;
    loop {
        match document.highlight(&mut queries, &[], std::time::Duration::from_millis(5), wall) {
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
    let failed = document.highlight(&mut Queries::new(), &[], LONG, wall);
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

/// `C-e` takes a comment's run for the whole comment (ADR 0111, step 6), so no capture may
/// paint inside one: a trailing comment is one run, whatever it holds.
#[test]
fn a_trailing_comment_is_one_run() {
    let rust = "fn f() { g(1); } // TODO: \"s\" 2 Some(x) fn\nlet x = 1; /// doc `y`\n";
    assert_has(
        Language::Rust,
        rust,
        &[
            ("// TODO: \"s\" 2 Some(x) fn", Comment),
            ("/// doc `y`", Comment),
        ],
    );
    let toml = "a = 1 # \"s\" true 2 [x]\n";
    assert_has(Language::Toml, toml, &[("# \"s\" true 2 [x]", Comment)]);
    let go = "func f() { g(1) } // TODO: \"s\" 2 nil f()\nvar x = 1 /* `y` */\n";
    assert_has(
        Language::Go,
        go,
        &[
            ("// TODO: \"s\" 2 nil f()", Comment),
            ("/* `y` */", Comment),
        ],
    );
    let ecma = "f(1); // TODO: \"s\" 2 null f() <div>\nlet x = 1; /** @param {string} y `z` */\n";
    for language in [Language::JavaScript, Language::TypeScript, Language::Tsx] {
        assert_has(
            language,
            ecma,
            &[
                ("// TODO: \"s\" 2 null f() <div>", Comment),
                ("/** @param {string} y `z` */", Comment),
            ],
        );
    }
}

/// A full-width space is no white space to Rust: before `//`, it leaves an error that takes the
/// marker in, so no comment is found and `C-e` ends where the text does (ADR 0111, step 6).
/// TOML keeps the comment.
#[test]
fn a_full_width_space_before_a_comment() {
    let rust = runs(&highlighted(
        Language::Rust,
        "fn f() {\n    let x = 1;\u{3000}// note\n}\n",
    ));
    assert!(rust.iter().all(|(_, h)| *h != Comment), "{rust:?}");
    assert_has(
        Language::Toml,
        "a = 1\u{3000}# note\n",
        &[("# note", Comment)],
    );
}
