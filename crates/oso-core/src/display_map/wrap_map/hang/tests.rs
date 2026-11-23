//! The tests of how far rows a line goes on in are indented, and of rows laid out with it.
//!
//! Must not depend on: windows or the renderer.

use proptest::prelude::*;

use super::*;
use crate::display_map::wrap_map::WrapSnapshot;
use crate::display_map::{DisplayMap, TabWidth};
use crate::rope::Rope;
use crate::sum_tree::Bias;

fn display(text: &str) -> DisplaySnapshot {
    DisplayMap::new(&Rope::from(text), TabWidth::new(4)).snapshot()
}

/// The indent of `line` in a window `columns` wide.
fn indent(line: &str, columns: usize) -> usize {
    Fit::new(columns, true).indent(&display(line), 0)
}

#[test]
fn a_line_goes_on_past_its_spaces_and_its_marks() {
    for (line, wanted) in [
        // A reply, its items, its quotes and its code.
        ("  text of a reply", 2),
        ("  - text", 4),
        ("  • text", 4),
        ("  1. text", 5),
        ("  12) text", 6),
        ("  │ • text", 6),
        ("      let code = 1;", 6),
        // Rows with oso's marks, of the conversation and nested in it.
        ("● text", 2),
        ("› text", 2),
        ("▸ ≡ Read · a.rs", 4),
        ("… $ Bash · cargo test", 4),
        ("  ▸ Bash cargo test", 4),
        ("│ ▸ ≡ Read · a.rs", 6),
        ("│   a subagent's reply", 4),
        ("│     a result line", 6),
        ("    12 + code", 4),
        // At least two columns, whatever the line starts with.
        ("Suggested: text", 2),
        ("text", 2),
        ("", 2),
        ("▸text", 2),
        ("2026 text", 2),
        ("1234567890. text", 2),
        ("- - - - - text", 8),
        // Only U+0020 is a space.
        ("\u{3000}\u{3000}text", 2),
        ("\t\ttext", 2),
        ("  \u{3000}text", 2),
        ("  \ttext", 2),
        ("  \u{a0}\u{a0}text", 2),
    ] {
        assert_eq!(indent(line, 101), wanted, "{line:?}");
    }
    // Wide, as a font with CJK draws it.
    assert_eq!(indent("〉 text", 101), 3);
}

#[test]
fn an_indent_is_at_most_half_a_row_and_never_less_than_a_nested_rows() {
    let deep = format!("{}text", " ".repeat(200));
    assert_eq!(indent(&deep, 101), 50);
    assert_eq!(indent(&deep, 41), 20);
    assert_eq!(indent(&deep, 13), 4, "room for the widest cluster");
    for columns in 13..200 {
        assert!(indent("  ▸ Bash cargo", columns) == 4, "{columns}");
        assert!(indent("text", columns) == 2, "{columns}");
    }
    // Narrower, nothing wraps, so no row goes on.
    assert!(!Fit::new(12, true).wraps());
    assert!(Fit::new(12, false).wraps());
    assert_eq!(Fit::new(101, false).indent(&display("  - text"), 0), 0);
}

/// Each row of `text` as a window draws it: its indent, then its text.
fn drawn(text: &str, columns: usize) -> Vec<String> {
    let display = display(text);
    let wrap = WrapSnapshot::hanging(&display, columns);
    (0..wrap.row_count())
        .map(|index| {
            let row = wrap.row(&display, index);
            let indent = match row.column {
                0 => 0,
                _ => wrap.indent(&display, row.line),
            };
            let shown = text[row.start..row.end].trim_end_matches('\n');
            format!("{}{shown}", " ".repeat(indent))
        })
        .collect()
}

#[test]
fn rows_line_up_under_their_lines_text() {
    let text = "  - one two three four five\n▸ Bash cargo test --all\n  日本語の文章です。長い。\n";
    assert_eq!(
        drawn(text, 17),
        [
            "  - one two ",
            "    three four ",
            "    five",
            "▸ Bash cargo ",
            "  test --all",
            "  日本語の文章で",
            "  す。長い。",
            "",
        ]
    );
}

#[test]
fn every_row_that_goes_on_is_indented_at_every_width() {
    // A reply's line, a nested call's and others, of words of many lengths and marks.
    let marks = [
        "▸ ", "▾ ", "◌ ", "● ", "› ", "〉 ", "· ", "∴ ", "✗ ", "… ", "│ ",
    ];
    for mark in marks {
        for (lead, least) in [("  ", 2), ("  ▸ Bash ", 4), ("Suggested: ", 2), ("● ", 2)] {
            for pad in 0..24 {
                let piece = format!("{}{mark}Bash rm -rf 日本語 ", "x".repeat(pad));
                let line = format!("{lead}{}", piece.repeat(12));
                let display = display(&line);
                for columns in [10, 11, 12, 13, 14, 20, 40, 60, 100] {
                    let wrap = WrapSnapshot::hanging(&display, columns);
                    for index in 1..wrap.row_count() {
                        let row = wrap.row(&display, index);
                        let indent = wrap.indent(&display, row.line);
                        assert!(indent >= least, "{line:?} at {columns}");
                        assert!(row.column >= indent, "{line:?} at {columns}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_row_does_not_break_before_where_the_next_one_stands() {
    // `あ` after `x` is the only place a break is allowed, a column short of the indent.
    let text = format!("xあ\u{a0}{}", "b".repeat(40));
    let display = display(&text);
    let wrap = WrapSnapshot::hanging(&display, 20);
    assert!(wrap.row_count() > 1);
    assert!(wrap.row(&display, 1).column >= 2);
}

#[test]
fn an_edit_of_what_a_line_starts_with_lays_the_whole_line_out_again() {
    let body = "word and more ".repeat(60);
    for (start, end, insert) in [(2, 2, "- "), (0, 2, ""), (0, 0, "│ "), (2, 2, "    ")] {
        let text = format!("first\n  {body}\n  {body}\n");
        let mut rope = Rope::from(text.as_str());
        let mut map = DisplayMap::new(&rope, TabWidth::new(4));
        let before = map.snapshot();
        let wrap = WrapSnapshot::hanging(&before, 40);
        let edit = rope.replace(6 + start..6 + end, insert);
        map.sync(&rope, &edit);
        let after = map.snapshot();
        let edited = wrap.edited(&before, &after, &edit);
        let fresh = WrapSnapshot::hanging(&after, 40);
        let rows = |wrap: &WrapSnapshot| -> Vec<usize> {
            (0..wrap.row_count())
                .map(|row| wrap.row(&after, row).start)
                .collect()
        };
        assert_eq!(rows(&edited), rows(&fresh), "{insert:?}");
    }
}

proptest! {
    /// A row fits in the width with its indent before it, a space that hangs aside, and starts
    /// no earlier on its line than it stands.
    #[test]
    fn prop_rows_fit_with_their_indent(
        text in "[ a-c│•あ、。\u{a0}\n1.-]{0,120}",
        columns in 13_usize..40,
    ) {
        let display = display(&text);
        let wrap = WrapSnapshot::hanging(&display, columns);
        for index in 0..wrap.row_count() {
            let row = wrap.row(&display, index);
            let indent = match row.column {
                0 => 0,
                _ => wrap.indent(&display, row.line),
            };
            prop_assert!(row.column >= indent);
            let end = match row.continues {
                true => display.offset_to_display_point(row.end).column,
                false => display.row_cells(row.line),
            };
            let cells = end - row.column + indent;
            prop_assert!(
                cells <= wrap.width() + 1,
                "{} cells at {}", cells, columns
            );
            let start = display.display_point_to_offset(
                DisplayPoint::new(row.line, row.column),
                Bias::Left,
            );
            prop_assert_eq!(start, row.start);
        }
    }
}
