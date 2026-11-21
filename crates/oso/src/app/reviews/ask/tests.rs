//! The tests of asking in the app: the rows a region spans, quoted with their signs and named
//! by their lines, cut to one file, and a row's text without its sign.
//!
//! Must not depend on: key presses, how a frame is drawn, git or the threads.

use std::sync::Arc;

use oso_core::review::{RowKind, Rows, Section, Side};
use oso_core::rope::Rope;

use super::{About, region_rows, row_text};

/// A file's section: its header, two unchanged lines from 4, the hunk's header, a removed
/// line 6 and two added lines 6 and 7; then another file's header.
fn rows() -> (Rows, Rope) {
    let mut a = Section::new(b"a.rs".as_slice().into());
    a.push(RowKind::File, 1);
    a.push(RowKind::Context { old: 4, new: 4 }, 2);
    a.push(RowKind::Hunk { hunk: 0 }, 1);
    a.push(
        RowKind::Lines {
            hunk: 0,
            side: Side::Old,
            first: 6,
        },
        1,
    );
    a.push(
        RowKind::Lines {
            hunk: 0,
            side: Side::New,
            first: 6,
        },
        2,
    );
    let mut b = Section::new(b"b.rs".as_slice().into());
    b.push(RowKind::File, 1);
    b.push(
        RowKind::Lines {
            hunk: 0,
            side: Side::New,
            first: 1,
        },
        1,
    );
    let rows = Rows::new(vec![Arc::new(a), Arc::new(b)]);
    let text =
        Rope::from("M a.rs\n four\n five\n@@ -6 +6,2 @@\n-six\n+SIX\n+seven\nA b.rs\n+one\n");
    (rows, text)
}

fn quoted(about: &About<'_>) -> (String, Vec<(char, String)>) {
    match about {
        About::Rows { place, lines } => (place.clone(), lines.clone()),
        _ => panic!("not rows"),
    }
}

#[test]
fn a_region_quotes_its_changed_and_unchanged_lines_named_by_the_new_side() {
    let (rows, text) = rows();
    let (path, about) = region_rows(&rows, &text, 0, 6).unwrap();
    assert_eq!(&*path, b"a.rs");
    let (place, lines) = quoted(&about);
    assert_eq!(place, "L4-L7 (new)");
    let lines: Vec<(char, &str)> = lines.iter().map(|(s, l)| (*s, l.as_str())).collect();
    assert_eq!(
        lines,
        vec![
            (' ', "four"),
            (' ', "five"),
            ('-', "six"),
            ('+', "SIX"),
            ('+', "seven"),
        ],
        "the header and the hunk's header are not quoted"
    );
}

#[test]
fn a_region_of_removed_lines_alone_is_named_by_the_old_side() {
    let (rows, text) = rows();
    let (_, about) = region_rows(&rows, &text, 4, 4).unwrap();
    assert_eq!(quoted(&about).0, "L6 (old)");
}

#[test]
fn a_region_stops_at_the_next_file() {
    let (rows, text) = rows();
    let (path, about) = region_rows(&rows, &text, 5, 8).unwrap();
    assert_eq!(&*path, b"a.rs");
    assert_eq!(quoted(&about).1.len(), 2, "b.rs's line is left out");
}

#[test]
fn a_region_with_no_line_quotes_nothing() {
    let (rows, text) = rows();
    assert!(region_rows(&rows, &text, 0, 0).is_none());
    assert!(region_rows(&rows, &text, 3, 3).is_none());
}

#[test]
fn a_row_s_text_leaves_its_sign_out_and_reads_a_bounded_length() {
    let text = Rope::from(format!("+{}\n", "x".repeat(10_000)).as_str());
    let line = row_text(&text, 0);
    assert!(line.starts_with('x'));
    assert!(line.len() < 3_000, "{}", line.len());
}
