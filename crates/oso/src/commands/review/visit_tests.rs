//! `RET` and `C-u RET` in a review: what each row asks the host to visit, in the unified text
//! and in a pane, and the rows that go nowhere.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; these run the command through the
//! registry on a review made here.

use oso_core::command::{Args, Prefix};
use oso_core::editor::{ReviewId, ReviewRequest};
use oso_core::frame::{FrameSize, Way};
use oso_core::review::Side;

use super::super::tests::{in_project, lines, put_at, put_panes, with_comments, with_review};
use crate::commands::tests::run;

/// What `RET`, or `C-u RET` with `old_side`, at `line` asked of the host.
fn visit_at(
    editor: &mut oso_core::editor::Editor,
    line: usize,
    old_side: bool,
) -> Vec<ReviewRequest> {
    put_at(editor, &[line]);
    let args = if old_side {
        Args::with_prefix(Some(Prefix::universal(None)))
    } else {
        Args::NONE
    };
    assert_eq!(run(editor, "review-visit", args).0, Ok(()), "line {line}");
    editor.take_review_requests()
}

fn visit(
    review: ReviewId,
    path: &[u8],
    (side, number): (Side, u32),
    old_side: bool,
) -> Vec<ReviewRequest> {
    vec![ReviewRequest::Visit {
        review,
        path: path.into(),
        side,
        number,
        old_side,
    }]
}

#[test]
fn each_row_visits_its_line_a_header_its_first_and_a_comment_the_line_it_is_on() {
    let (mut editor, project) = in_project(true);
    let (review, _) = with_comments(&mut editor, project);
    let cases = [
        // A removed line is asked by its own side: the host finds where it was removed.
        (lines::OLD, (Side::Old, 1)),
        (lines::NEW_2, (Side::New, 2)),
        (lines::HUNK, (Side::New, 1)),
        (lines::FILE, (Side::New, 1)),
        // The person's comment and a reply in its thread are on new line 1, the agent's on
        // the hunk, whose first new line is 1.
        (lines::MINE, (Side::New, 1)),
        (lines::MINE_CARD, (Side::New, 1)),
        (lines::MY_REPLY, (Side::New, 1)),
        (lines::AGENTS_CARD, (Side::New, 1)),
    ];
    for (line, at) in cases {
        assert_eq!(
            visit_at(&mut editor, line, false),
            visit(review, b"a", at, false),
            "line {line}"
        );
    }
    // `b` has no reading in the model: its line still says itself.
    assert_eq!(
        visit_at(&mut editor, lines::B_LINE, false),
        visit(review, b"b", (Side::New, 1), false)
    );
    // `C-u RET`: a header goes to the old side's first line, a hunk to its old start.
    assert_eq!(
        visit_at(&mut editor, lines::HUNK, true),
        visit(review, b"a", (Side::Old, 1), true)
    );
    assert_eq!(
        visit_at(&mut editor, lines::FILE, true),
        visit(review, b"a", (Side::Old, 1), true)
    );
    assert_eq!(
        visit_at(&mut editor, lines::NEW_2, true),
        visit(review, b"a", (Side::New, 2), true)
    );
}

#[test]
fn the_title_and_a_filler_row_go_nowhere_and_outside_a_review_nothing_runs() {
    let (mut editor, project) = in_project(true);
    assert_eq!(
        run(&mut editor, "review-visit", Args::NONE).0,
        Err("Not in a review".to_owned())
    );
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 200,
    });
    let review = with_review(&mut editor, project);
    put_at(&mut editor, &[lines::TITLE]);
    assert_eq!(
        run(&mut editor, "review-visit", Args::NONE).0,
        Err("Nothing to visit here".to_owned())
    );
    put_panes(&mut editor, review);
    let window = editor.window().id();
    editor.show_review_panes(window, Way::Across).unwrap();
    editor.select_window(window);
    // The old pane: a's removed line is of its side; the row after it is a filler.
    assert_eq!(
        visit_at(&mut editor, 3, false),
        visit(review, b"a", (Side::Old, 1), false)
    );
    put_at(&mut editor, &[4]);
    assert_eq!(
        run(&mut editor, "review-visit", Args::NONE).0,
        Err("Nothing to visit here".to_owned())
    );
    assert!(editor.take_review_requests().is_empty());
    // The new pane: its second line.
    let new = editor.pair_of(window).unwrap();
    editor.select_window(new);
    assert_eq!(
        visit_at(&mut editor, 4, false),
        visit(review, b"a", (Side::New, 2), false)
    );
}

#[test]
fn ret_is_bound_and_listed_in_the_review_s_keys() {
    assert!(super::super::KEYS.contains(&("RET", "review-visit")));
    let listed = super::super::keys::keys_text("t", super::super::KEYS, "f");
    assert!(listed.contains("review-visit"), "{listed}");
    assert!(
        listed.contains("Open the file of the review's line"),
        "{listed}"
    );
}
