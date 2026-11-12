//! Tests of where `C-e` finds a line's code ends: trailing comments, comment markers inside
//! strings, comments that do not end the line, comment-only lines, blanks of every kind, no
//! highlights, clusters, and what it may read (ADR 0058, ADR 0111).
//!
//! Must not depend on: `buffer`, `window` or `editor`.

use super::super::{MAX_STOP_WALK, WalkBudget, cluster_start, line_end, line_start};
use super::{code_end, toggle_line_end};
use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use crate::rope::Rope;
use crate::selection::Selection;
use crate::syntax::{Highlight, Highlights};
use proptest::prelude::*;

fn display(text: &str) -> DisplaySnapshot {
    DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
}

/// `text`'s highlights: each of `parts`, found in it, as what it says, the rest plain.
fn highlights(text: &str, parts: &[(&str, Highlight)]) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    for (part, highlight) in parts {
        let at = text.find(part).expect("a part of the text");
        highlights.replace(at..at + part.len(), [(part.len(), Some(*highlight))]);
    }
    highlights
}

/// Where the code of `display`'s line `row` ends, as `C-e` finds it, from `budget`.
fn code_end_of(
    display: &DisplaySnapshot,
    row: usize,
    highlights: Option<&Highlights>,
    budget: &mut WalkBudget,
) -> usize {
    let end = line_end(display, row);
    code_end(display, (row, end), highlights, &mut budget.stop())
}

/// Where the code of `text`'s line `row` ends, with `parts` highlighted.
fn end(text: &str, row: usize, parts: &[(&str, Highlight)]) -> usize {
    let highlights = highlights(text, parts);
    code_end_of(
        &display(text),
        row,
        Some(&highlights),
        &mut WalkBudget::new(),
    )
}

#[test]
fn a_trailing_comment_ends_the_code_before_it_and_its_blanks() {
    let text = "let x = 1;  // note  ";
    assert_eq!(end(text, 0, &[("// note  ", Highlight::Comment)]), 10);
    // TOML's comment, and blanks of every kind between code and comment.
    let text = "x = 1\t\u{3000} # note";
    assert_eq!(end(text, 0, &[("# note", Highlight::Comment)]), 5);
}

#[test]
fn a_comment_marker_in_a_string_is_no_comment() {
    let text = "let s = \"a // b\";";
    let parts = [("\"a // b\"", Highlight::String)];
    assert_eq!(end(text, 0, &parts), text.len());
}

#[test]
fn a_comment_that_does_not_end_the_line_is_passed() {
    let text = "f(/* a */ b);";
    assert_eq!(end(text, 0, &[("/* a */", Highlight::Comment)]), text.len());
}

#[test]
fn a_block_comment_that_goes_on_stops_the_code_on_its_first_line_only() {
    // Line breaks are never highlighted, so the comment is a run on each line.
    let text = "x(); /* start\n  mid\n*/ y();\n";
    let parts = [
        ("/* start", Highlight::Comment),
        ("  mid", Highlight::Comment),
        ("*/", Highlight::Comment),
    ];
    assert_eq!(end(text, 0, &parts), 4);
    assert_eq!(end(text, 1, &parts), "x(); /* start\n  mid".len());
    assert_eq!(end(text, 2, &parts), "x(); /* start\n  mid\n*/ y();".len());
}

#[test]
fn a_line_of_only_a_comment_ends_where_its_text_does() {
    let text = "    // note\n/// doc  \n";
    let parts = [
        ("// note", Highlight::Comment),
        ("/// doc  ", Highlight::Comment),
    ];
    assert_eq!(end(text, 0, &parts), "    // note".len());
    assert_eq!(end(text, 1, &parts), "    // note\n/// doc".len());
}

#[test]
fn with_no_highlights_the_code_ends_where_the_text_does() {
    let text = "f(); // c  \r\nnext";
    let none = code_end_of(&display(text), 0, None, &mut WalkBudget::new());
    assert_eq!(none, "f(); // c".len());
    // A CRLF line break ends the line as LF does.
    assert_eq!(end(text, 0, &[("// c  ", Highlight::Comment)]), 4);
}

#[test]
fn c_e_goes_between_the_code_end_and_the_line_end() {
    let text = "f(); // c\ng(); // d\n";
    let display = display(text);
    let parts = [("// c", Highlight::Comment), ("// d", Highlight::Comment)];
    let highlights = highlights(text, &parts);
    // Where `C-e` with count `n` takes a cursor at `from`.
    let press = |from: usize, n: isize| {
        let mut selection = Selection::at(from);
        let budget = &mut WalkBudget::new();
        toggle_line_end(&display, &mut selection, n, Some(&highlights), budget);
        selection.cursor
    };
    // Two stops, as mwim's: the code's end, then the line's, then the code's again.
    assert_eq!([press(0, 1), press(4, 1), press(9, 1)], [4, 9, 4]);
    // From inside the comment, back to the code's end.
    assert_eq!(press(7, 1), 4);
    // With a count, the code's end of the line it moves to.
    assert_eq!(press(0, 2), 14);
}

#[test]
fn a_comment_that_starts_inside_a_cluster_ends_the_code_at_a_clusters_start() {
    // A prepended character joins what follows it into one cluster, here with the first `/`.
    let text = "\u{d4e}// c";
    assert_eq!(end(text, 0, &[("// c", Highlight::Comment)]), text.len());
    let text = "x \u{d4e}// c";
    assert_eq!(end(text, 0, &[("// c", Highlight::Comment)]), 1);
}

/// What a line is made of: code, blanks, comment markers and characters that join clusters.
fn piece() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "a",
        "/",
        " ",
        "\t",
        "\u{3000}",
        "\n",
        "\r\n",
        "\u{d4e}",
        "\u{600}",
        "\u{301}",
        "\u{200d}",
        "\u{1f468}",
        "\u{915}\u{94d}",
        "日",
    ])
}

proptest! {
    /// Wherever highlighting puts comments, the code's end is on its row, and where a cursor
    /// can be.
    #[test]
    fn the_code_ends_on_its_row_at_a_clusters_start(
        pieces in prop::collection::vec(piece(), 0..24),
        comments in prop::collection::vec((0usize..64, 1usize..16), 0..4),
    ) {
        let text: String = pieces.concat();
        let mut highlights = Highlights::new(text.len());
        for (at, len) in comments {
            let at = text.floor_char_boundary(at.min(text.len()));
            let end = text.floor_char_boundary((at + len).min(text.len()));
            highlights.replace(at..end, [(end - at, Some(Highlight::Comment))]);
        }
        let display = display(&text);
        for row in 0..display.row_count() {
            let end = code_end_of(&display, row, Some(&highlights), &mut WalkBudget::new());
            prop_assert!(line_start(&display, row) <= end && end <= line_end(&display, row));
            prop_assert_eq!(cluster_start(&display, end), end);
        }
    }
}

#[test]
fn both_walks_back_are_one_stops_reads() {
    // Blanks after the comment and between it and the code, each under a stop's reads and
    // together over them.
    let blanks = " ".repeat(MAX_STOP_WALK * 3 / 4);
    let text = format!("x;{blanks}// c{blanks}");
    let display = display(&text);
    let highlights = highlights(&text, &[("// c", Highlight::Comment)]);
    let mut budget = WalkBudget::new();
    let before = budget.left();
    code_end_of(&display, 0, Some(&highlights), &mut budget);
    assert!(
        before - budget.left() <= MAX_STOP_WALK,
        "{}",
        before - budget.left()
    );
}

#[test]
fn a_spent_command_reads_nothing_more() {
    let text = "x; // c   ";
    let display = display(text);
    let highlights = highlights(text, &[("// c", Highlight::Comment)]);
    let mut budget = WalkBudget::with_limit(0);
    let end = code_end_of(&display, 0, Some(&highlights), &mut budget);
    assert_eq!(end, text.len(), "where it got to: nowhere");
    assert!(!budget.is_spent(), "it tried to read nothing");
}

#[test]
fn a_stop_spent_on_the_blanks_after_a_comment_reads_no_highlights() {
    // The walk back over the trailing blanks takes all of a stop's reads, so the code ends
    // where it got to, among the blanks, not before the comment those blanks are part of.
    let blanks = " ".repeat(MAX_STOP_WALK * 2);
    let text = format!("x; // c{blanks}");
    let display = display(&text);
    let highlights = highlights(&text, &[(&format!("// c{blanks}"), Highlight::Comment)]);
    let end = code_end_of(&display, 0, Some(&highlights), &mut WalkBudget::new());
    assert!(end > "x; // c".len(), "{end}");
}

/// Rust's grammar finds no comment after a full-width space (oso-syntax's
/// `a_full_width_space_before_a_comment`), so `C-e` ends where the text does.
#[test]
fn after_a_full_width_space_rust_finds_no_comment_and_the_code_ends_at_the_text() {
    let text = "let x = 1;\u{3000}// note";
    assert_eq!(end(text, 0, &[("1", Highlight::Constant)]), text.len());
}
