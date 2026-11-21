//! A review's question: its first lines, a line's and a hunk's place and lines, a region's
//! rows, an unchanged line, a file, a secret file, fences longer than the lines' backticks,
//! lines that cannot become several once pasted, and cuts; the rows a region spans, quoted
//! with their signs and named by their lines, cut to one file, and a row's text without its
//! sign.
//!
//! Must not depend on: `app`, `dispatch` or winit; these compose over shapes made here.

use std::sync::Arc;

use oso_core::review::{Anchor, Base, Hunk, RowKind, Rows, Section, Shape, Side};
use oso_core::rope::Rope;
use proptest::prelude::*;

use super::{About, HEAD, Where, compose, region_rows, row_text};
use crate::review::message::{CUT_TO, MAX_HUNK_LINES};

const PATH: &[u8] = b"src/a.rs";

const REVIEW: Where<'static> = Where {
    scope: "uncommitted, in oso",
    worktree: "/work/oso-wt",
};

fn hunk(old_start: u32, new_start: u32, removed: &[&str], added: &[&str]) -> Hunk {
    let lines = |lines: &[&str]| {
        (lines.iter())
            .map(|line| format!("{line}\n").into_bytes().into_boxed_slice())
            .collect()
    };
    Hunk {
        old_start,
        new_start,
        removed: lines(removed),
        added: lines(added),
    }
}

fn shape(hunks: Vec<Hunk>) -> Shape {
    Shape::new(Base::Unknown, hunks).unwrap()
}

fn rows(lines: &[(char, &str)]) -> Vec<(char, String)> {
    (lines.iter())
        .map(|(sign, line)| (*sign, (*line).to_owned()))
        .collect()
}

#[test]
fn a_line_goes_with_the_review_its_place_its_hunk_and_the_question() {
    let shape = shape(vec![hunk(2, 2, &["old"], &["new", "more"])]);
    let about = About::Hunk {
        anchor: Anchor::Line {
            side: Side::New,
            number: 3,
        },
        shape: &shape,
    };
    let text = compose(&REVIEW, PATH, &about, false, "Why two lines?");
    assert_eq!(
        text,
        format!(
            "{HEAD}\nReview: uncommitted, in oso. Worktree: /work/oso-wt.\n\n## src/a.rs  L3 \
             (new)\n```diff\n@@ -2 +2,2 @@\n-old\n+new\n+more\n```\nQuestion: Why two lines?"
        )
    );
}

#[test]
fn a_hunk_s_header_goes_with_the_lines_it_spans() {
    let shape = shape(vec![hunk(10, 10, &["a"], &["b", "c", "d"])]);
    let about = About::Hunk {
        anchor: Anchor::Hunk { index: 0 },
        shape: &shape,
    };
    let text = compose(&REVIEW, PATH, &about, false, "Q");
    assert!(
        text.contains("## src/a.rs  L10-L12 (new)\n```diff\n@@"),
        "{text}"
    );
}

#[test]
fn a_region_goes_as_its_rows_under_their_place() {
    let about = About::Rows {
        place: "L4-L6 (new)".to_owned(),
        lines: rows(&[(' ', "keep"), ('-', "gone"), ('+', "came")]),
    };
    let text = compose(&REVIEW, PATH, &about, false, "Q");
    assert!(
        text.ends_with("## src/a.rs  L4-L6 (new)\n```diff\n keep\n-gone\n+came\n```\nQuestion: Q"),
        "{text}"
    );
}

#[test]
fn a_file_goes_by_its_name_alone() {
    let text = compose(&REVIEW, PATH, &About::File, false, "Q");
    assert!(
        text.ends_with("## src/a.rs  (the file)\nQuestion: Q"),
        "{text}"
    );
}

#[test]
fn a_file_that_may_hold_secrets_sends_its_place_alone() {
    let shape = shape(vec![hunk(1, 1, &["KEY=old"], &["KEY=new"])]);
    let about = About::Hunk {
        anchor: Anchor::Hunk { index: 0 },
        shape: &shape,
    };
    let text = compose(&REVIEW, b".env", &about, true, "Q");
    assert!(
        text.contains("## .env  L1 (new)\n(The file may hold secrets"),
        "{text}"
    );
    assert!(!text.contains("KEY="), "{text}");
    let about = About::Rows {
        place: "L1 (new)".to_owned(),
        lines: rows(&[('+', "KEY=new")]),
    };
    let text = compose(&REVIEW, b".env", &about, true, "Q");
    assert!(!text.contains("KEY="), "{text}");
}

#[test]
fn the_fence_is_longer_than_any_run_of_backticks_in_the_lines() {
    let about = About::Rows {
        place: "L1 (new)".to_owned(),
        lines: rows(&[('+', "a ```` b")]),
    };
    let text = compose(&REVIEW, PATH, &about, false, "Q");
    assert!(text.contains("`````diff\n+a ```` b\n`````\n"), "{text}");
}

#[test]
fn a_long_region_is_cut_to_its_first_lines() {
    let many: Vec<(char, String)> = (0..=MAX_HUNK_LINES)
        .map(|n| ('+', format!("line {n}")))
        .collect();
    let about = About::Rows {
        place: "L1-L61 (new)".to_owned(),
        lines: many,
    };
    let text = compose(&REVIEW, PATH, &about, false, "Q");
    assert!(
        text.contains(&format!("+line {}\n```", CUT_TO - 1)),
        "{text}"
    );
    assert!(!text.contains(&format!("+line {CUT_TO}\n")), "{text}");
    assert!(text.contains("past the first 30 are left out"), "{text}");
}

#[test]
fn paths_and_scopes_are_escaped_so_they_read_as_one_line() {
    let review = Where {
        scope: "uncommitted, in a\nb",
        worktree: "/w/x\ny",
    };
    let text = compose(&review, b"a\n## b.rs", &About::File, false, "Q");
    assert_eq!(
        text.lines().filter(|line| line.starts_with("## ")).count(),
        1,
        "{text}"
    );
    assert_eq!(text.lines().count(), 5, "{text}");
}

proptest! {
    /// No line of a region becomes several once pasted, whatever it holds, and no line of it
    /// closes the fence.
    #[test]
    fn a_region_s_lines_stay_lines_inside_their_fence(
        lines in proptest::collection::vec("[a-z`\r\u{2028}\u{1b} ]{0,40}", 1..8)
    ) {
        let about = About::Rows {
            place: "L1 (new)".to_owned(),
            lines: lines.iter().map(|line| ('+', line.clone())).collect(),
        };
        let text = compose(&REVIEW, PATH, &about, false, "Q");
        let fence = text.lines().find(|line| line.ends_with("diff")).unwrap();
        let fence = fence.strip_suffix("diff").unwrap();
        let body: Vec<&str> = text
            .split('\n')
            .skip_while(|line| !line.ends_with("diff"))
            .skip(1)
            .take_while(|line| *line != fence)
            .collect();
        prop_assert_eq!(body.len(), lines.len(), "{}", text);
        prop_assert!(!text.contains('\r'), "{}", text);
        prop_assert!(!text.contains('\u{2028}'), "{}", text);
    }
}

/// A file's section: its header, two unchanged lines from 4, the hunk's header, a removed
/// line 6 and two added lines 6 and 7; then another file's header.
fn section_rows() -> (Rows, Rope) {
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
    let (rows, text) = section_rows();
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
    let (rows, text) = section_rows();
    let (_, about) = region_rows(&rows, &text, 4, 4).unwrap();
    assert_eq!(quoted(&about).0, "L6 (old)");
}

#[test]
fn a_region_stops_at_the_next_file() {
    let (rows, text) = section_rows();
    let (path, about) = region_rows(&rows, &text, 5, 8).unwrap();
    assert_eq!(&*path, b"a.rs");
    assert_eq!(quoted(&about).1.len(), 2, "b.rs's line is left out");
}

#[test]
fn a_region_with_no_line_quotes_nothing() {
    let (rows, text) = section_rows();
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

#[test]
fn a_long_region_reads_only_its_first_lines_and_is_named_to_its_end() {
    let mut a = Section::new(b"a.rs".as_slice().into());
    a.push(RowKind::File, 1);
    a.push(
        RowKind::Lines {
            hunk: 0,
            side: Side::New,
            first: 1,
        },
        500,
    );
    let rows = Rows::new(vec![Arc::new(a)]);
    let mut text = String::from("A a.rs\n");
    for n in 1..=500 {
        text.push_str(&format!("+line {n}\n"));
    }
    let text = Rope::from(text.as_str());
    let (_, about) = region_rows(&rows, &text, 0, 500).unwrap();
    let (place, lines) = quoted(&about);
    assert_eq!(place, "L1-L500 (new)");
    assert_eq!(
        lines.len(),
        61,
        "one more than a whole hunk's, so that the cut is known"
    );
}

#[test]
fn the_question_s_invisible_and_control_characters_are_escaped_and_it_is_not_cut() {
    let long = "x".repeat(2_000);
    let question = format!("a\u{202e}b\rc {long}");
    let text = compose(&REVIEW, PATH, &About::File, false, &question);
    assert!(
        text.ends_with(&format!("Question: a\\u{{202e}}b\\u{{d}}c {long}")),
        "{text}"
    );
}
