//! Tests of where a line's code ends: trailing comments, comment markers inside strings,
//! comments that do not end the line, comment-only lines, blanks of every kind and no
//! highlights (ADR 0111).
//!
//! Must not depend on: `buffer`, `window` or `editor`.

use super::super::{WalkBudget, toggle_line_end};
use super::code_end;
use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use crate::rope::Rope;
use crate::selection::Selection;
use crate::syntax::{Highlight, Highlights};

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

/// Where the code of `text`'s line `row` ends, with `parts` highlighted.
fn end(text: &str, row: usize, parts: &[(&str, Highlight)]) -> usize {
    let highlights = highlights(text, parts);
    code_end(
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
    let none = code_end(&display(text), 0, None, &mut WalkBudget::new());
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
        let n = (n, Some(&highlights));
        toggle_line_end(&display, &mut selection, n, &mut WalkBudget::new());
        selection.cursor
    };
    // Two stops, as mwim's: the code's end, then the line's, then the code's again.
    assert_eq!([press(0, 1), press(4, 1), press(9, 1)], [4, 9, 4]);
    // From inside the comment, back to the code's end.
    assert_eq!(press(7, 1), 4);
    // With a count, the code's end of the line it moves to.
    assert_eq!(press(0, 2), 14);
}
