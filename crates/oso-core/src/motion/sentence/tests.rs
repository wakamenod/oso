//! Tests of motion by sentences: English ends with one space, closing quotes and brackets,
//! Japanese ends with no space, sentences a paragraph's end cuts, counts, the buffer's edges
//! and what a step may read (ADR 0190).
//!
//! Must not depend on: `buffer`, `window` or `editor`.

use super::super::text::testing::{display, marked};
use super::super::{WalkBudget, cluster_start};
use super::forward_sentence;
use crate::motion::Edge;
use crate::selection::Selection;
use proptest::prelude::*;

/// Where one sentence at a time from `from` stops in `text`, in the way `n` goes, until the
/// buffer's edge.
fn stops(text: &str, from: usize, n: isize) -> Vec<usize> {
    let d = display(text);
    let mut selection = Selection::at(from);
    let mut stops = Vec::new();
    let mut budget = WalkBudget::new();
    while forward_sentence(&d, &mut selection, n, &mut budget).is_ok() {
        assert!(stops.len() < 64, "{stops:?}");
        stops.push(selection.cursor);
    }
    assert!(!budget.is_spent());
    stops
}

/// Checks that `forward` and `backward`, the same text with a `|` at each stop forward from
/// its start and back from its end, have those stops.
fn check(forward: &str, backward: &str) {
    let (text, ahead) = marked(forward);
    let (same, mut behind) = marked(backward);
    assert_eq!(text, same);
    assert_eq!(stops(&text, 0, 1), ahead, "forward in {text:?}");
    behind.reverse();
    assert_eq!(stops(&text, text.len(), -1), behind, "back in {text:?}");
}

#[test]
fn one_space_after_a_mark_ends_a_sentence() {
    check("One.| Two?| Three!| Four|", "|One. |Two? |Three! |Four");
    check("A.|  B.|", "|A.  |B.");
    check("Wait…| what|", "|Wait… |what");
}

#[test]
fn closing_quotes_and_brackets_go_with_the_sentence() {
    check(
        "He said \"hi.\"| Then (ok.)| ‘Fine.’| end.|",
        "|He said \"hi.\" |Then (ok.) |‘Fine.’ |end.",
    );
    check("«Oui.»| [Yes!]| Done.|", "|«Oui.» |[Yes!] |Done.");
}

#[test]
fn a_mark_with_no_space_after_it_ends_nothing() {
    check("3.14 is pi.| Yes|", "|3.14 is pi. |Yes");
    check("a.b?c!d|", "|a.b?c!d");
}

#[test]
fn a_line_end_after_a_mark_ends_a_sentence() {
    check("End.|\nNext.|", "|End.\n|Next.");
    check("One.|\r\nTwo.|", "|One.\r\n|Two.");
}

#[test]
fn japanese_marks_end_a_sentence_with_no_space() {
    check(
        "今日は晴れ。|明日は雨！|本当？|はい．|",
        "|今日は晴れ。|明日は雨！|本当？|はい．",
    );
    check("本当！？|次|", "|本当！？|次");
    check("文。| Next.|", "|文。 |Next.");
}

#[test]
fn a_paragraphs_end_ends_a_sentence() {
    check(
        "First line\nwithout end|\n\nSecond para.|",
        "|First line\nwithout end\n\n|Second para.",
    );
    check("One.| Two|\n\nThree.|", "|One. |Two\n\n|Three.");
}

#[test]
fn from_between_two_sentences() {
    let text = "One.  Two.";
    // Inside the spaces after a sentence: back to its start, on to the next one's end.
    assert_eq!(stops(text, 5, -1), [0]);
    assert_eq!(stops(text, 5, 1), [10]);
    // From the blank lines between paragraphs.
    let text = "One.\n\n\nTwo.";
    assert_eq!(stops(text, 6, 1), [11]);
    assert_eq!(stops(text, 6, -1), [0]);
}

#[test]
fn counts_go_on_and_the_edges_stop_them() {
    let text = "One. Two? Three! Four";
    let d = display(text);
    let mut budget = WalkBudget::new();
    let mut selection = Selection::at(0);
    assert_eq!(forward_sentence(&d, &mut selection, 2, &mut budget), Ok(()));
    assert_eq!(selection.cursor, 9);
    assert_eq!(
        forward_sentence(&d, &mut selection, -1, &mut budget),
        Ok(())
    );
    assert_eq!(selection.cursor, 5);
    assert_eq!(
        forward_sentence(&d, &mut selection, 9, &mut budget),
        Err(Edge::End)
    );
    assert_eq!(selection.cursor, text.len());
    assert_eq!(
        forward_sentence(&d, &mut selection, -9, &mut budget),
        Err(Edge::Start)
    );
    assert_eq!(selection.cursor, 0);
    assert!(stops("", 0, 1).is_empty());
    assert!(stops("", 0, -1).is_empty());
}

#[test]
fn a_step_past_its_budget_stays() {
    let text = "word ".repeat(2048);
    let d = display(&text);
    for (from, n) in [(0, 1), (text.len(), -1)] {
        let mut selection = Selection::at(from);
        let mut budget = WalkBudget::with_limit(4096);
        let _ = forward_sentence(&d, &mut selection, n, &mut budget);
        assert!(budget.is_spent());
        assert_eq!(selection.cursor, from);
    }
}

#[test]
fn a_whole_budget_crosses_a_megabyte_paragraph() {
    let text = format!("{}.", "word ".repeat(1 << 18));
    let d = display(&text);
    let mut selection = Selection::at(0);
    let _ = forward_sentence(&d, &mut selection, 1, &mut WalkBudget::new());
    assert_eq!(selection.cursor, text.len());
    let _ = forward_sentence(&d, &mut selection, -1, &mut WalkBudget::new());
    assert_eq!(selection.cursor, 0);
}

/// Pieces of text that sentences read.
fn piece() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "a", " ", "\n", "\n\n", "\r\n", ".", "?", "\"", ")", "。", "！", "\u{3099}", "\u{301}",
        "\u{200d}", "\t", "…", "あ",
    ])
}

proptest! {
    /// Every stop is where a cursor can be, and each step goes the way it is told.
    #[test]
    fn stops_are_cluster_starts_in_order(
        pieces in prop::collection::vec(piece(), 0..40),
        from in 0usize..80,
    ) {
        let text: String = pieces.concat();
        let d = display(&text);
        let from = cluster_start(&d, text.floor_char_boundary(from.min(text.len())));
        for n in [1, -1] {
            let mut at = from;
            loop {
                let mut selection = Selection::at(at);
                let mut budget = WalkBudget::new();
                let edge = forward_sentence(&d, &mut selection, n, &mut budget);
                prop_assert!(!budget.is_spent());
                let to = selection.cursor;
                if edge.is_err() {
                    prop_assert_eq!(to, at);
                    break;
                }
                prop_assert!(if n > 0 { to > at } else { to < at }, "{} to {} in {:?}", at, to, text);
                prop_assert_eq!(cluster_start(&d, to), to, "{:?}", text);
                at = to;
            }
        }
    }
}
