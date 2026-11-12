//! Tests of painting: inner over outer, the first pattern on one span, line breaks, cutting
//! and deep nesting (ADR 0111).
//!
//! Must not depend on: the worker; these call the module directly.

use oso_core::rope::Rope;
use oso_core::syntax::Highlight::{self, Comment, Keyword, String, Type};

use super::{Capture, paint};

fn capture(range: std::ops::Range<usize>, pattern: usize, class: Highlight) -> Capture {
    Capture {
        range,
        layer: 0,
        pattern,
        class,
    }
}

#[test]
fn an_inner_capture_paints_over_an_outer_one() {
    let text = Rope::from("aaaaaaaaaa");
    let runs = paint(
        &text,
        0..10,
        vec![capture(0..10, 1, Comment), capture(3..5, 0, Keyword)],
    );
    assert_eq!(
        runs,
        [(3, Some(Comment)), (2, Some(Keyword)), (5, Some(Comment))]
    );
}

#[test]
fn of_the_same_span_the_first_pattern_wins() {
    let text = Rope::from("FOO");
    let first = paint(
        &text,
        0..3,
        vec![capture(0..3, 1, Type), capture(0..3, 0, Keyword)],
    );
    assert_eq!(first, [(3, Some(Keyword))]);
    let swapped = paint(
        &text,
        0..3,
        vec![capture(0..3, 0, Type), capture(0..3, 1, Keyword)],
    );
    assert_eq!(swapped, [(3, Some(Type))]);
}

#[test]
fn of_the_same_span_an_injected_grammar_wins() {
    let text = Rope::from("abc");
    let mut inner = capture(0..3, 5, String);
    inner.layer = 1;
    let runs = paint(&text, 0..3, vec![inner, capture(0..3, 0, Keyword)]);
    assert_eq!(runs, [(3, Some(String))]);
}

#[test]
fn line_breaks_stay_plain() {
    let text = Rope::from("/* a\nb\n\nc */\n");
    let runs = paint(&text, 0..text.len(), vec![capture(0..12, 0, Comment)]);
    assert_eq!(
        runs,
        [
            (4, Some(Comment)),
            (1, None),
            (1, Some(Comment)),
            (2, None),
            (4, Some(Comment)),
            (1, None),
        ]
    );
}

#[test]
fn line_breaks_stay_plain_across_chunks() {
    let line = "x".repeat(700);
    let text = Rope::from(format!("{line}\n{line}\n{line}").as_str());
    let runs = paint(
        &text,
        0..text.len(),
        vec![capture(0..text.len(), 0, String)],
    );
    assert_eq!(
        runs,
        [
            (700, Some(String)),
            (1, None),
            (700, Some(String)),
            (1, None),
            (700, Some(String)),
        ]
    );
}

#[test]
fn captures_are_cut_to_the_piece() {
    let text = Rope::from("0123456789");
    let runs = paint(
        &text,
        4..8,
        vec![capture(0..5, 0, Comment), capture(7..20, 0, String)],
    );
    assert_eq!(runs, [(1, Some(Comment)), (2, None), (1, Some(String))]);
}

#[test]
fn deep_nesting_paints_in_one_sweep() {
    let depth = 100_000;
    let text = Rope::from("(".repeat(2 * depth).as_str());
    let captures = (0..depth)
        .map(|level| capture(level..2 * depth - level, 0, [Type, Keyword][level % 2]))
        .collect();
    let runs = paint(&text, 0..2 * depth, captures);
    assert_eq!(runs.len(), 2 * depth - 1);
    assert_eq!(runs.iter().map(|(len, _)| len).sum::<usize>(), 2 * depth);
}
