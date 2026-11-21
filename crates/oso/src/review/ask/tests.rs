//! A review's question: its first lines, a line's and a hunk's place and lines, a region's
//! rows, an unchanged line, a file, a secret file, fences longer than the lines' backticks,
//! lines that cannot become several once pasted, and cuts.
//!
//! Must not depend on: `app`, `dispatch` or winit; these compose over shapes made here.

use oso_core::review::{Anchor, Base, Hunk, Shape, Side};
use proptest::prelude::*;

use super::{About, HEAD, Review, compose};
use crate::review::message::{CUT_TO, MAX_HUNK_LINES};

const PATH: &[u8] = b"src/a.rs";

const REVIEW: Review<'static> = Review {
    scope: "uncommitted, in oso",
    root: "/work/oso-wt",
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
    let review = Review {
        scope: "uncommitted, in a\nb",
        root: "/w/x\ny",
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
