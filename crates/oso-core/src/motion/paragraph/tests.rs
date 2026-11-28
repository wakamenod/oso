//! Tests of motion by paragraphs: blank lines with spaces, form feeds, Markdown headings and
//! fences, counts, the buffer's edges, `mark-paragraph`'s bounds and what a step may read
//! (ADR 0190).
//!
//! Must not depend on: `buffer`, `window` or `editor`.

use super::super::WalkBudget;
use super::super::text::testing::{display, marked};
use super::{forward_paragraph, paragraph_bounds};
use crate::motion::Edge;
use crate::selection::Selection;
use proptest::prelude::*;

/// Where one paragraph at a time from `from` stops in `text`, `n` at a time, until the
/// buffer's edge.
fn stops(text: &str, markdown: bool, from: usize, n: isize) -> Vec<usize> {
    let d = display(text);
    let mut selection = Selection::at(from);
    let mut stops = Vec::new();
    let mut budget = WalkBudget::new();
    while forward_paragraph(&d, markdown, &mut selection, n, &mut budget).is_ok() {
        assert!(stops.len() < 64, "{stops:?}");
        stops.push(selection.cursor);
    }
    assert!(!budget.is_spent());
    stops
}

/// Checks that `forward` and `backward`, `text` with a `|` at each stop forward from its start
/// and back from its end, have the same text and those stops.
fn check(forward: &str, backward: &str, markdown: bool) {
    let (text, ahead) = marked(forward);
    let (same, mut behind) = marked(backward);
    assert_eq!(text, same);
    assert_eq!(stops(&text, markdown, 0, 1), ahead, "forward in {text:?}");
    behind.reverse();
    let back = stops(&text, markdown, text.len(), -1);
    assert_eq!(back, behind, "back in {text:?}");
}

#[test]
fn blank_lines_with_spaces_separate_paragraphs() {
    check(
        "one\ntwo\n|  \t\nthree\n|\n\nfour|",
        "|one\ntwo\n|  \t\nthree\n\n|\nfour",
        false,
    );
}

#[test]
fn forward_stops_at_the_first_blank_line_and_back_at_the_last() {
    check("a\n|\n\n\nb|", "|a\n\n\n|\nb", false);
    // From the blank lines: forward to the next paragraph's end, back to the one before's
    // start.
    let (text, _) = marked("a\n\n\n\nb");
    assert_eq!(stops(&text, false, 3, 1), [6]);
    assert_eq!(stops(&text, false, 3, -1), [0]);
}

#[test]
fn a_form_feed_separates_or_starts_a_paragraph() {
    // A line of a form feed alone separates.
    check("a\n|\u{c}\nb|", "|a\n|\u{c}\nb", false);
    // A line that starts with one starts a paragraph, with no line between.
    check("a\n|\u{c}b\nc|", "|a\n|\u{c}b\nc", false);
    // A form feed after a space does not.
    check(" \u{c}b\nc|", "| \u{c}b\nc", false);
}

#[test]
fn markdown_headings_and_fences_stand_apart() {
    let text = "# Title\ntext\nmore\n```\ncode\n```\nafter";
    check(
        "# Title\n|text\nmore\n|```\n|code\n|```\n|after|",
        "|# Title\n|text\nmore\n|```\n|code\n|```\n|after",
        true,
    );
    // Not in other modes.
    assert_eq!(stops(text, false, 0, 1), [text.len()]);
    // With blank lines too.
    check(
        "intro\n|\n## Part\n|\nbody|",
        "|intro\n|\n## Part\n|\nbody",
        true,
    );
    check("~~~ sh\n|ls\n|~~~|", "|~~~ sh\n|ls\n|~~~", true);
}

#[test]
fn a_markdown_heading_is_a_few_marks_and_a_space() {
    for heading in ["# a", "###### a", "   ## a", "#", "##\t", "# a\r"] {
        let text = format!("x\n{heading}\ny");
        let after = text.len() - 1;
        assert_eq!(
            stops(&text, true, 0, 1),
            [2, after, text.len()],
            "{heading:?}"
        );
    }
    for text_line in ["#tag", "####### a", "    # a", "a # b"] {
        let text = format!("x\n{text_line}\ny");
        assert_eq!(stops(&text, true, 0, 1), [text.len()], "{text_line:?}");
    }
}

#[test]
fn line_breaks_of_two_characters() {
    check("a\r\nb\r\n|\r\nc|", "|a\r\nb\r\n|\r\nc", false);
}

#[test]
fn counts_go_on_and_negative_ones_go_back() {
    let text = "a\n\nb\n\nc\n\nd";
    let d = display(text);
    let mut budget = WalkBudget::new();
    let mut selection = Selection::at(0);
    assert_eq!(
        forward_paragraph(&d, false, &mut selection, 2, &mut budget),
        Ok(())
    );
    assert_eq!(selection.cursor, 5);
    assert_eq!(
        forward_paragraph(&d, false, &mut selection, -1, &mut budget),
        Ok(())
    );
    assert_eq!(selection.cursor, 2);
    assert_eq!(
        forward_paragraph(&d, false, &mut selection, 0, &mut budget),
        Ok(())
    );
    assert_eq!(selection.cursor, 2);
}

#[test]
fn the_buffer_edges_stop_a_count() {
    let text = "a\n\nb";
    let d = display(text);
    let mut budget = WalkBudget::new();
    let mut selection = Selection::at(0);
    assert_eq!(
        forward_paragraph(&d, false, &mut selection, 9, &mut budget),
        Err(Edge::End)
    );
    assert_eq!(selection.cursor, text.len());
    assert_eq!(
        forward_paragraph(&d, false, &mut selection, -9, &mut budget),
        Err(Edge::Start)
    );
    assert_eq!(selection.cursor, 0);
    assert!(stops("", false, 0, 1).is_empty());
    assert!(stops("", false, 0, -1).is_empty());
}

#[test]
fn a_motion_clears_the_goal_and_keeps_the_mark() {
    let d = display("a\n\nb");
    let mut selection = Selection {
        cursor: 0,
        mark: Some(3),
        goal: Some(crate::selection::Goal::Column(4)),
    };
    let _ = forward_paragraph(&d, false, &mut selection, 1, &mut WalkBudget::new());
    assert_eq!(
        selection,
        Selection {
            cursor: 2,
            mark: Some(3),
            goal: None
        }
    );
}

#[test]
fn the_bounds_take_the_line_above() {
    let text = "a\n\nb\nc\n\nd";
    let d = display(text);
    let mut budget = WalkBudget::new();
    assert_eq!(paragraph_bounds(&d, 3, false, &mut budget), 2..7);
    assert_eq!(paragraph_bounds(&d, 0, false, &mut budget), 0..2);
    assert_eq!(paragraph_bounds(&d, 9, false, &mut budget), 7..9);
    assert_eq!(paragraph_bounds(&d, 8, false, &mut budget), 7..9);
    let (text, marks) = marked("x\n|# H\n|y");
    let d = display(&text);
    assert_eq!(
        paragraph_bounds(&d, 3, true, &mut budget),
        marks[0]..marks[1]
    );
    assert!(!budget.is_spent());
}

#[test]
fn a_step_past_its_budget_stays() {
    let text = "a\n".repeat(4096);
    let d = display(&text);
    let mut selection = Selection::at(0);
    let mut budget = WalkBudget::with_limit(4096);
    let _ = forward_paragraph(&d, false, &mut selection, 1, &mut budget);
    assert!(budget.is_spent());
    assert_eq!(selection.cursor, 0);
    let mut selection = Selection::at(text.len());
    let mut budget = WalkBudget::with_limit(4096);
    let _ = forward_paragraph(&d, false, &mut selection, -1, &mut budget);
    assert!(budget.is_spent());
    assert_eq!(selection.cursor, text.len());
}

#[test]
fn a_count_keeps_the_steps_it_could_pay_for() {
    let text = format!("a\n\n{}", "b\n".repeat(4096));
    let d = display(&text);
    let mut selection = Selection::at(0);
    let mut budget = WalkBudget::with_limit(4096);
    let _ = forward_paragraph(&d, false, &mut selection, 2, &mut budget);
    assert!(budget.is_spent());
    assert_eq!(selection.cursor, 2);
}

#[test]
fn a_whole_budget_crosses_a_megabyte_of_lines() {
    let text = "ab\n".repeat(1 << 20);
    let d = display(&text);
    let mut selection = Selection::at(0);
    let mut budget = WalkBudget::new();
    let _ = forward_paragraph(&d, false, &mut selection, 1, &mut budget);
    assert!(!budget.is_spent());
    assert_eq!(selection.cursor, text.len());
    let _ = forward_paragraph(&d, false, &mut selection, -1, &mut WalkBudget::new());
    assert_eq!(selection.cursor, 0);
}

/// Pieces of text that paragraphs read.
fn piece() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "a", "\n", " ", "\t", "\u{c}", "\r", "\r\n", "#", "# ", "```", "~", "あ", "\u{301}", "é",
    ])
}

proptest! {
    /// Every stop is a line's start or the buffer's end, and each step goes the way it is told.
    #[test]
    fn stops_are_line_starts_in_order(
        pieces in prop::collection::vec(piece(), 0..40),
        markdown in any::<bool>(),
        from in 0usize..80,
    ) {
        let text: String = pieces.concat();
        let d = display(&text);
        let from = super::super::cluster_start(&d, text.floor_char_boundary(from.min(text.len())));
        for n in [1, -1] {
            let mut at = from;
            loop {
                let mut selection = Selection::at(at);
                let mut budget = WalkBudget::new();
                let edge = forward_paragraph(&d, markdown, &mut selection, n, &mut budget);
                prop_assert!(!budget.is_spent());
                let to = selection.cursor;
                if edge.is_err() {
                    prop_assert_eq!(to, at);
                    break;
                }
                prop_assert!(if n > 0 { to > at } else { to < at }, "{} to {} in {:?}", at, to, text);
                let line_start = to == 0 || text.as_bytes()[to - 1] == b'\n';
                prop_assert!(line_start || to == text.len(), "{} in {:?}", to, text);
                at = to;
            }
        }
    }
}
