//! Tests of an excerpt a list's preview shows (ADR 0114, ADR 0138): its bounds, and its place
//! on the row's line however its column is counted.
//!
//! Must not depend on: threads or the file system.

use super::{EXCERPT_LINE, EXCERPT_LINES, Excerpt, PreviewAt, byte_of_column};
use crate::editor::VisitColumn::{self, Chars, Utf8, Utf16};

fn excerpt_text(excerpt: &Excerpt) -> String {
    excerpt.text.to_string()
}

#[test]
fn an_excerpt_keeps_its_lines_cut_and_the_rows_line_around_its_match() {
    let long = "x".repeat(EXCERPT_LINE + 100);
    let far = format!("{}needle tail", "y".repeat(3 * EXCERPT_LINE));
    let mut bytes = format!("before\r\n{long}\n{far}\nafter").into_bytes();
    // An invalid byte in the last line.
    bytes.extend_from_slice(b"\xff\n");
    let excerpt = Excerpt::new(&bytes, 10, (12, Chars(3 * EXCERPT_LINE)), Some((1, 2)));
    let text = excerpt_text(&excerpt);
    let lines: Vec<&str> = text.split('\n').collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0], "before", "the \\r before the break goes");
    assert_eq!(lines[1], format!("{}…", "x".repeat(EXCERPT_LINE)));
    assert!(lines[2].starts_with('…') && lines[2].len() <= EXCERPT_LINE + 6);
    assert!(text[excerpt.cursor..].starts_with("needle tail"));
    assert_eq!(lines[3], "after\u{fffd}");
    assert_eq!(excerpt.identity, Some((1, 2)));
    let debug = format!("{excerpt:?}");
    assert!(!debug.contains("needle") && !debug.contains("before"));
}

#[test]
fn an_excerpt_counts_columns_as_a_row_does_and_keeps_at_most_its_lines() {
    // Each invalid byte is one column, as a row counts it.
    let excerpt = Excerpt::new(b"a\xff\xffb needle\n", 1, (1, Chars(5)), None);
    let text = excerpt_text(&excerpt);
    assert!(text[excerpt.cursor..].starts_with("needle"), "{text:?}");
    let many = "line\n".repeat(EXCERPT_LINES * 2);
    let excerpt = Excerpt::new(many.as_bytes(), 1, (EXCERPT_LINES + 5, Chars(0)), None);
    assert_eq!(excerpt.text.line_count(), EXCERPT_LINES);
    // A line past what was kept puts the cursor at the end.
    assert_eq!(excerpt.cursor, excerpt.text.len());
}

#[test]
fn a_column_is_found_as_its_server_counts_it_and_clamps_at_the_line_s_end() {
    // `é` is two bytes and one UTF-16 unit; `𝄞` four bytes and two units.
    let line = "aé𝄞b".as_bytes();
    let b = line.len() - 1;
    for column in [Chars(3), Utf8(7), Utf16(4)] {
        assert_eq!(byte_of_column(line, column), b, "{column:?}");
    }
    // Inside a character is at its start.
    assert_eq!(byte_of_column(line, Utf8(2)), 1);
    assert_eq!(byte_of_column(line, Utf16(3)), 3);
    for column in [Chars(99), Utf8(99), Utf16(99)] {
        assert_eq!(byte_of_column(line, column), line.len(), "{column:?}");
    }
    // Each invalid byte is one of each.
    for column in [Chars(2), Utf8(2), Utf16(2)] {
        assert_eq!(byte_of_column(b"\xff\xffz", column), 2, "{column:?}");
    }
}

#[test]
fn an_excerpt_puts_its_place_by_a_server_s_column() {
    let excerpt = Excerpt::new("x\n\t𝄞 needle\n".as_bytes(), 1, (2, Utf16(4)), None);
    let text = excerpt_text(&excerpt);
    assert!(text[excerpt.cursor..].starts_with("needle"), "{text:?}");
    let at = |column: VisitColumn| PreviewAt::Line { line: 2, column };
    assert_eq!((at(Utf8(3)).line(), at(Utf8(3)).column()), (2, Utf8(3)));
    let offset = PreviewAt::Offset {
        offset: 9,
        line: 4,
        column: 2,
    };
    assert_eq!((offset.line(), offset.column()), (4, Chars(2)));
}
