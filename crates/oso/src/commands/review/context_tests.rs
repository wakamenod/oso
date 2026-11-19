//! Which hidden lines the context keys show, by the rows alone: the gap each takes, which end
//! of it, how many lines, and the files and rows that have none that way.
//!
//! Must not depend on: the host's threads; the rows are made here.

use std::sync::Arc;

use oso_core::review::{RowKind, Rows, Section, Side};

use super::*;

/// A file with a gap of `top` lines before its hunk at old line `top + 4`, three lines of
/// context around the hunk, and a gap of 40 after; then another file with no gap.
fn rows(top: u32) -> Rows {
    let mut a = Section::new(b"a.rs"[..].into());
    a.push(RowKind::File, 1);
    a.push(
        RowKind::Gap {
            old: 1,
            new: 1,
            lines: top,
        },
        1,
    );
    a.push(
        RowKind::Context {
            old: top + 1,
            new: top + 1,
        },
        3,
    );
    a.push(RowKind::Hunk { hunk: 0 }, 1);
    let hunk = top + 4;
    a.push(
        RowKind::Lines {
            hunk: 0,
            side: Side::Old,
            first: hunk,
        },
        1,
    );
    a.push(
        RowKind::Context {
            old: hunk + 1,
            new: hunk + 1,
        },
        3,
    );
    a.push(
        RowKind::Gap {
            old: hunk + 4,
            new: hunk + 4,
            lines: 40,
        },
        1,
    );
    let mut b = Section::new(b"b.rs"[..].into());
    b.push(RowKind::File, 1);
    b.push(RowKind::Hunk { hunk: 0 }, 1);
    Rows::new(vec![Arc::new(a), Arc::new(b)])
}

const TOP_GAP: usize = 1;
const HUNK: usize = 5;
const LAST_CONTEXT: usize = 9;
const BOTTOM_GAP: usize = 10;

fn path(range: Range<u32>) -> Option<(Box<[u8]>, Range<u32>)> {
    Some((b"a.rs"[..].into(), range))
}

#[test]
fn off_a_gap_each_key_shows_the_hidden_lines_next_to_what_is_shown() {
    let rows = rows(100);
    // Above the hunk: the last 20 of the 100, just above its context.
    assert_eq!(shown_by(&rows, HUNK, Toward::Up, STEP), path(81..101));
    // Below it: the first 20 of the 40 after.
    assert_eq!(shown_by(&rows, HUNK, Toward::Down, STEP), path(108..128));
    assert_eq!(
        shown_by(&rows, LAST_CONTEXT, Toward::Down, 5),
        path(108..113)
    );
    assert_eq!(shown_by(&rows, HUNK, Toward::Whole, MOST), path(108..148));
}

#[test]
fn on_a_gap_row_up_shows_its_first_lines_and_down_its_last() {
    let rows = rows(100);
    assert_eq!(shown_by(&rows, TOP_GAP, Toward::Up, STEP), path(1..21));
    assert_eq!(shown_by(&rows, TOP_GAP, Toward::Down, STEP), path(81..101));
    assert_eq!(shown_by(&rows, TOP_GAP, Toward::Whole, MOST), path(1..101));
    assert_eq!(
        shown_by(&rows, BOTTOM_GAP, Toward::Down, STEP),
        path(128..148)
    );
}

#[test]
fn a_gap_no_larger_than_the_count_shows_whole_and_a_count_is_bounded() {
    let rows = rows(7);
    assert_eq!(shown_by(&rows, HUNK, Toward::Up, STEP), path(1..8));
    assert_eq!(shown_by(&rows, HUNK, Toward::Up, 0), path(7..8));
    let rows = Rows::new(vec![{
        let mut a = Section::new(b"a.rs"[..].into());
        a.push(RowKind::File, 1);
        a.push(
            RowKind::Gap {
                old: 1,
                new: 1,
                lines: 50_000,
            },
            1,
        );
        Arc::new(a)
    }]);
    assert_eq!(shown_by(&rows, 1, Toward::Up, u32::MAX), path(1..1 + MOST));
    assert_eq!(shown_by(&rows, 1, Toward::Whole, MOST), path(1..1 + MOST));
}

#[test]
fn no_gap_of_another_file_is_taken() {
    let rows = rows(100);
    // The second file's hunk: the gap above it is the first file's.
    assert_eq!(shown_by(&rows, BOTTOM_GAP + 2, Toward::Up, STEP), None);
    assert_eq!(shown_by(&rows, BOTTOM_GAP + 2, Toward::Down, STEP), None);
    // The first file's header has a gap below, none above.
    assert_eq!(shown_by(&rows, 0, Toward::Up, STEP), None);
    assert_eq!(shown_by(&rows, 0, Toward::Down, STEP), path(1..21));
}

#[test]
fn a_prefix_argument_sets_the_count() {
    assert_eq!(count_of(Args::NONE), STEP);
    assert_eq!(count_of(Args::numeric(5)), 5);
    assert_eq!(count_of(Args::numeric(-5)), 5);
    assert_eq!(count_of(Args::with_prefix(Some(Prefix::Universal(4)))), 80);
    assert_eq!(
        count_of(Args::with_prefix(Some(Prefix::Universal(16)))),
        320
    );
    assert_eq!(count_of(Args::numeric(isize::MAX)), MOST);
}

/// A review in the selected window whose text has a line for each row of [`rows`].
fn editor_with_rows(top: u32) -> (oso_core::editor::Editor, ReviewId) {
    use oso_core::load::decode_all;
    use oso_core::rope::Rope;
    use oso_core::syntax::Highlights;

    let (mut editor, project) = super::super::tests::in_project(true);
    let (review, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
    let window = editor.window().id();
    editor.show_in(window, buffer);
    let rows = rows(top);
    let text = "x\n".repeat(rows.lines());
    let loaded = decode_all(text.as_bytes(), Default::default()).compared_with(&Rope::new());
    let _ = editor.set_review_text(
        review,
        loaded,
        rows,
        Highlights::new(text.len()),
        &|_, _, number| Some(number),
    );
    (editor, review)
}

#[test]
fn each_cursor_asks_once_for_the_lines_its_key_shows() {
    use super::super::tests::put_at;
    use crate::commands::tests::run;

    let (mut editor, review) = editor_with_rows(100);
    put_at(&mut editor, &[HUNK, LAST_CONTEXT, BOTTOM_GAP]);
    assert_eq!(
        run(&mut editor, "review-context-down", Args::NONE).0,
        Ok(())
    );
    let asked = editor.take_review_requests();
    let context = |show: Option<Range<u32>>| ReviewRequest::Context {
        review,
        path: b"a.rs"[..].into(),
        show,
    };
    // The hunk and its context take the gap below's first lines, once; the gap's row its last.
    assert_eq!(asked, [context(Some(108..128)), context(Some(128..148))]);
    assert_eq!(
        run(&mut editor, "review-reset-context", Args::NONE).0,
        Ok(())
    );
    assert_eq!(editor.take_review_requests(), [context(None)]);
    // The second file has no gap.
    put_at(&mut editor, &[BOTTOM_GAP + 2]);
    assert_eq!(
        run(&mut editor, "review-context-up", Args::numeric(3)).0,
        Err("No hidden lines above".into())
    );
    assert!(editor.take_review_requests().is_empty());
}

#[test]
fn no_comment_goes_on_a_context_line_or_a_gap() {
    use super::super::tests::put_at;
    use crate::commands::tests::run;

    let (mut editor, _) = editor_with_rows(100);
    for line in [TOP_GAP, HUNK - 1] {
        put_at(&mut editor, &[line]);
        assert_eq!(
            run(&mut editor, "review-comment", Args::NONE).0,
            Err("Comments go on a changed line or a hunk's header".into())
        );
    }
}
