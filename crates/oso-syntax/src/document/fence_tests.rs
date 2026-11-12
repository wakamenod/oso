//! Tests of Markdown's fenced code: which fences take a grammar, in containers too, the cap, a
//! fence's own lines, edits inside and around a fence, and many fences (ADR 0111).
//!
//! Must not depend on: the worker; these call a document directly.

use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_core::syntax::Highlight::{self, Comment, Constant, Function, Keyword, Markup, Property};
use oso_core::syntax::Splice;

use super::fence::MAX_FENCE_BYTES;
use super::highlight_tests::{highlighted, runs};
use super::tests::LONG;
use super::{Document, Failure, Highlighted};
use crate::language::Language;
use crate::queries::Queries;

type Runs = Vec<(Range<usize>, Option<Highlight>)>;

fn all_runs(document: &Document) -> Runs {
    let highlights = document.highlights();
    highlights.runs(0..highlights.len()).collect()
}

/// Whether `text`'s Markdown has `part` as `highlight`, as a whole run.
fn has(text: &str, part: &str, highlight: Highlight) -> bool {
    runs(&highlighted(Language::Markdown, text))
        .iter()
        .any(|(run, found)| run == part && *found == highlight)
}

/// Whether the first `part` of `text`'s Markdown is plain throughout.
fn plain(text: &str, part: &str) -> bool {
    let document = highlighted(Language::Markdown, text);
    let at = text.find(part).expect("a part of the text");
    document
        .highlights()
        .runs(at..at + part.len())
        .all(|(_, highlight)| highlight.is_none())
}

#[test]
fn rust_and_toml_fences_take_their_grammars() {
    let text = "Some text.\n\n```rust\nfn main() {} // c\n```\n\n~~~toml\na = 1 # c\n~~~\n\n\
                ```RS\nlet x = 7;\n```\n\n```rust,ignore\nfn g() {}\n```\n\n```rust{.x}\nfn h() {}\n```\n";
    for (part, highlight) in [
        ("main", Function),
        ("// c", Comment),
        ("a", Property),
        ("# c", Comment),
        ("let", Keyword),
        ("7", Constant),
        ("h", Function),
        ("g", Function),
    ] {
        assert!(has(text, part, highlight), "{part:?} as {highlight:?}");
    }
}

#[test]
fn a_fences_delimiters_and_info_string_are_markup() {
    let text = "```rust\nfn f() {}\n```\n\n~~~ python\nx\n~~~\n";
    assert!(has(text, "```rust", Markup));
    assert!(has(text, "~~~", Markup));
    assert!(has(text, "python", Markup));
}

#[test]
fn other_fences_and_indented_code_stay_plain() {
    for text in [
        "```\nfn f() {}\n```\n",
        "```python\nfn f() {}\n```\n",
        "```{.rust}\nfn f() {}\n```\n",
        "```markdown\nfn f() {}\n```\n",
        "```rustacean\nfn f() {}\n```\n",
        "Text.\n\n    fn f() {}\n",
    ] {
        assert!(plain(text, "fn f() {}"), "{text:?}");
    }
    // TOML front matter too.
    assert!(plain("+++\na = 1\n+++\n\nText.\n", "a = 1"));
}

#[test]
fn a_fence_in_a_list_or_a_quote_leaves_out_its_continuations() {
    let list = "- item\n\n  ```rust\n  fn f() {\n      g();\n  }\n  ```\n";
    assert!(has(list, "f", Function));
    assert!(has(list, "g", Function));
    let quote = "> ```toml\n> a = 1\n> b = \"s\"\n> ```\n";
    assert!(has(quote, "a", Property));
    assert!(has(quote, "\"s\"", Highlight::String));
    assert!(
        has(quote, "> ", Markup),
        "a continuation stays the block grammar's"
    );
}

#[test]
fn a_fence_past_the_cap_stays_plain() {
    let line = "fn f() {}\n";
    let fence = |lines: usize| {
        format!(
            "```rust\n{}```\n\n```rust\nfn g() {{}}\n```\n",
            line.repeat(lines)
        )
    };
    // The cap counts the delimiter lines too.
    let under = fence((MAX_FENCE_BYTES - 16) / line.len());
    assert!(has(&under, "f", Function));
    let over = fence(MAX_FENCE_BYTES / line.len() + 1);
    assert!(plain(&over, "fn f() {}"), "past the cap");
    assert!(
        has(&over, "```rust", Markup),
        "its delimiters are still markup"
    );
    assert!(has(&over, "g", Function), "the next fence");
}

/// `before` highlighted, then edited by replacing `old` bytes at `at` with `new`, and
/// highlighted again, has the highlights of a fresh document of the text after.
#[track_caller]
fn edit_matches_fresh(before: &str, at: usize, old: usize, new: &str) {
    let mut document = highlighted(Language::Markdown, before);
    let after = format!("{}{new}{}", &before[..at], &before[at + old..]);
    let splice = Splice {
        start: at,
        old_len: old,
        new_len: new.len(),
    };
    document.edit(&[splice], &Rope::from(after.as_str()));
    let done = document.highlight(&mut Queries::new(), &[], LONG);
    assert_eq!(done, Highlighted::Done);
    let fresh = highlighted(Language::Markdown, &after);
    assert_eq!(all_runs(&document), all_runs(&fresh), "{after:?}");
}

#[test]
fn edits_inside_and_around_a_fence_highlight_it_again() {
    let text = "# T\n\n```rust\nfn f() {}\nfn g() {}\nfn h() {}\n```\n\nText *a*.\n";
    let content = text.find("fn f").unwrap();
    // A block comment opened on the first line reaches past it, to the fence's end.
    edit_matches_fresh(text, content, 0, "/* ");
    // The info string names another language, then none oso injects.
    let info = text.find("rust").unwrap();
    edit_matches_fresh(text, info, 4, "toml");
    edit_matches_fresh(text, info, 4, "text");
    // The opening fence goes, and its content is a paragraph.
    edit_matches_fresh(text, info - 3, 7, "");
    // The closing fence goes, and the fence runs to the end.
    let close = text.rfind("```").unwrap();
    edit_matches_fresh(text, close, 4, "");
    // A new fence opens above.
    edit_matches_fresh(text, 4, 0, "```\n");
}

#[test]
fn a_fence_grown_past_the_cap_is_plain_again() {
    let text = "```rust\nfn f() {}\n```\n";
    let inside = text.find('}').unwrap() + 1;
    let grown = format!("\n{}", "fn g() {}\n".repeat(MAX_FENCE_BYTES / 10));
    edit_matches_fresh(text, inside, 0, &grown);
}

/// A fence of crafted nesting at the cap is highlighted, or fails as too slow, within the
/// limits; so is a text of many tiny fences, each parsed on its own.
#[test]
fn crafted_and_many_fences_stay_within_the_limits() {
    let crafted = "(".repeat(MAX_FENCE_BYTES - 1);
    let many = "```rust\nx\n```\n~~~toml\na=1\n~~~\n".repeat((256 << 10) / 29);
    for text in [format!("```rust\n{crafted}\n```\n"), many] {
        let mut document = Document::new(Language::Markdown, Rope::from(text.as_str()));
        let mut queries = Queries::new();
        let started = Instant::now();
        let done = loop {
            match document.highlight(&mut queries, &[], Duration::from_millis(20)) {
                Highlighted::Paused => continue,
                done => break done,
            }
        };
        let took = started.elapsed();
        assert!(
            matches!(
                done,
                Highlighted::Done | Highlighted::Failed(Failure::TooSlow)
            ),
            "{done:?}"
        );
        // The total limits, with room for a debug build.
        assert!(took < Duration::from_secs(10), "{took:?}");
        assert_eq!(document.highlights().len(), text.len());
    }
}

/// Fences that follow each other stay apart in what the document keeps of them, through
/// edits too, so that a fence painted again leaves its neighbours alone.
#[test]
fn fences_that_follow_each_other_stay_apart() {
    let fence = "```rust\nfn f() {}\n```\n";
    let text = fence.repeat(4);
    let mut document = highlighted(Language::Markdown, &text);
    // A key in the second fence's content.
    let at = fence.len() + "```rust\nfn f".len();
    let after = format!("{}x{}", &text[..at], &text[at..]);
    let splice = Splice {
        start: at,
        old_len: 0,
        new_len: 1,
    };
    document.edit(&[splice], &Rope::from(after.as_str()));
    // Mapped through the edit, they still touch, and stay four.
    assert_eq!(
        document.fences.ranges().len(),
        4,
        "{:?}",
        document.fences.ranges()
    );
    let done = document.highlight(&mut Queries::new(), &[], LONG);
    assert_eq!(done, Highlighted::Done);
    let fences = document.fences.ranges().to_vec();
    assert_eq!(fences.len(), 4, "{fences:?}");
    document.repainted(&fences[1], &fences[1..2]);
    assert!(
        document.pending.ranges().is_empty(),
        "{:?}",
        document.pending.ranges()
    );
}

/// Inside a quote or a list item, a fence ends after the next line's continuation, where the
/// next fence starts. A key in the middle one of five, each too large for its neighbours to
/// share a piece, is highlighted to the end, as a fresh document is, and soon.
#[test]
fn a_key_among_fences_in_a_container_ends() {
    let code = "let x = 1;\n".repeat(900);
    for prefix in ["> ", "  "] {
        let fence = || {
            let body: String = code
                .lines()
                .map(|line| format!("{prefix}{line}\n"))
                .collect();
            format!("{prefix}```rust\n{body}{prefix}```\n")
        };
        let lead = if prefix == "> " { "" } else { "- item\n\n" };
        let text = format!("{lead}{}", (0..5).map(|_| fence()).collect::<String>());
        // The first statement past the middle, in the middle fence.
        let at = text.len() / 2;
        let at = at + text[at..].find("let").unwrap();
        let started = Instant::now();
        edit_matches_fresh(&text, at, 3, "fn ");
        let took = started.elapsed();
        assert!(took < Duration::from_secs(2), "{prefix:?}: {took:?}");
    }
}
