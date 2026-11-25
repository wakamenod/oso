//! A review shown in the layout over another's two panes (ADR 0122, step 11; ADR 0167): it takes
//! the whole area the pair had, so that the judge gives it two panes too, and the slot, the
//! commit list and a window the person split stay.
//!
//! Must not depend on: the app's threads, git or files; these work on an editor made here.

use std::sync::Arc;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{CommitEntry, Commits, Editor, ReviewArea, ReviewId, ReviewLayout};
use oso_core::frame::{FrameSize, Way};

use oso_core::window::{RowMap, WindowId};

use super::{enter, show_laid, windows};
use crate::app::reviews::judge::SIDE_BY_SIDE;
use crate::commands::review::tests::{in_project, lines_of, put_at, put_panes, with_review};
use crate::sessions::spaces::kept::{self, Kept};

/// A frame wide enough for two panes beside the slot, the layout in it and the first review in
/// it as a pair laid out `way`.
struct Laid {
    editor: Editor,
    kept: Vec<Kept>,
    first: ReviewId,
    second: ReviewId,
    second_buffer: BufferId,
    left: WindowId,
    right: WindowId,
    slot: WindowId,
}

fn laid(way: Way) -> Laid {
    let (mut editor, project) = in_project(true);
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 300,
    });
    let first = with_review(&mut editor, project);
    let (second, second_buffer) = editor.add_review(project, "*review: commit 1 oso*".into());
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, first));
    let left = editor.window().id();
    let slot = windows(&editor)[1].0;
    put_panes(&mut editor, first);
    editor.show_review_panes(left, way).unwrap();
    let right = editor.pair_of(left).unwrap();
    Laid {
        editor,
        kept,
        first,
        second,
        second_buffer,
        left,
        right,
        slot,
    }
}

fn areas(editor: &Editor) -> Vec<ReviewArea> {
    let mut areas = Vec::new();
    editor.review_areas(&mut areas);
    areas
}

/// The second review alone, in the cells the pair had, the slot beside it.
fn assert_second_whole(laid: &Laid, before: &[ReviewArea]) {
    let now = areas(&laid.editor);
    assert_eq!(now.len(), 1, "{now:?}");
    assert_eq!(now[0].review, laid.second);
    assert_eq!(now[0].layout, ReviewLayout::Unified);
    assert_eq!(now[0].columns, before[0].columns);
    assert!(now[0].columns >= SIDE_BY_SIDE, "wide enough for two panes");
    let shown = windows(&laid.editor);
    assert!(shown.iter().any(|(id, ..)| *id == laid.slot), "the slot");
    assert_eq!(kept::review(&laid.kept).unwrap().slot, Some(laid.slot));
}

#[test]
fn a_review_shown_over_another_s_panes_takes_the_pair_s_whole_area() {
    for way in [Way::Across, Way::Down] {
        for from_right in [false, true] {
            let mut laid = laid(way);
            let before = areas(&laid.editor);
            assert_eq!(before[0].layout, ReviewLayout::Panes(way));
            laid.editor
                .select_window(if from_right { laid.right } else { laid.left });
            assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.second).0);
            assert_second_whole(&laid, &before);
            assert_eq!(windows(&laid.editor).len(), 2, "{way:?}");
            assert_eq!(laid.editor.window().id(), laid.left);
            assert_eq!(laid.editor.window().buffer(), laid.second_buffer);
        }
    }
}

#[test]
fn the_pair_s_right_window_may_be_the_layout_s_own() {
    let mut laid = laid(Way::Across);
    let before = areas(&laid.editor);
    kept::review_mut(&mut laid.kept).unwrap().window = laid.right;
    assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.second).0);
    assert_second_whole(&laid, &before);
    assert_eq!(laid.editor.window().id(), laid.right);
}

#[test]
fn the_place_in_the_panes_is_where_the_review_shows_again() {
    let mut laid = laid(Way::Across);
    // a's second new line, in the new pane, beside a filler row.
    laid.editor.select_window(laid.right);
    put_at(&mut laid.editor, &[4]);
    assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.second).0);
    assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.first).0);
    assert_eq!(laid.editor.window().id(), laid.left);
    assert_eq!(lines_of(&laid.editor), [5], "`+z` in the one stream");
}

#[test]
fn the_commit_list_and_a_window_split_by_hand_stay() {
    let mut laid = laid(Way::Across);
    let entry = CommitEntry {
        short: "aaaaaaa".into(),
        subject: "s".into(),
        author: "Ann".into(),
        date: "2026-11-20".into(),
        merge: false,
        empty: false,
        unpushed: false,
        viewed: 0,
        hunks: 0,
        unsent: 0,
        new: false,
    };
    let commits = Commits {
        number: 12,
        entries: Arc::from(vec![entry]),
        more: false,
        shown: None,
        open: vec![(0, laid.second)],
        since: None,
    };
    laid.editor.set_review_commits(laid.first, Some(commits));
    let list = (laid.editor)
        .show_review_commits(laid.first, laid.left)
        .unwrap();
    // Split by hand, it shows the pane alone.
    laid.editor.select_window(laid.slot);
    let by_hand = laid.editor.split_window(Way::Down, false).unwrap();
    let pane = (laid.editor.window_by_id(laid.right)).unwrap().buffer();
    laid.editor.show_in(by_hand, pane);
    let before = areas(&laid.editor);
    laid.editor.select_window(list);
    assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.second).0);
    // As `SPC` does, the list is selected again.
    laid.editor.select_window(list);
    assert_eq!(laid.editor.window().id(), list);
    let ids: Vec<_> = (windows(&laid.editor).iter()).map(|(id, ..)| *id).collect();
    for stays in [list, laid.left, laid.slot, by_hand] {
        assert!(ids.contains(&stays));
    }
    assert!(!ids.contains(&laid.right));
    let now = areas(&laid.editor);
    let area = now.iter().find(|area| area.window == laid.left).unwrap();
    assert_eq!(
        (area.review, area.columns),
        (laid.second, before[0].columns)
    );
}

#[test]
fn only_a_pair_of_a_review_s_panes_is_undone() {
    let mut laid = laid(Way::Across);
    let notes = laid.editor.add_buffer(Buffer::from_text("notes", "x\n"));
    let more = laid.editor.add_buffer(Buffer::from_text("more", "y\n"));
    laid.editor.select_window(laid.slot);
    laid.editor.show(notes);
    let other = (laid.editor)
        .pair_window(Way::Down, more, RowMap::same())
        .unwrap();
    assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.second).0);
    assert_eq!(laid.editor.pair_of(laid.slot), Some(other));
    // As a show outside the layout asks of the selected window.
    assert!(!laid.editor.show_review_unified(laid.slot));
    assert_eq!(laid.editor.pair_of(laid.slot), Some(other));
}

#[test]
fn the_review_shown_already_shows_in_its_whole_area() {
    let mut laid = laid(Way::Across);
    let before = areas(&laid.editor);
    assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.first).0);
    let now = areas(&laid.editor);
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].columns, before[0].columns);
    // Its panes are there still, for the host to show at once.
    assert!(laid.editor.review_panes(laid.first).is_some());
    laid.editor
        .show_review_panes(laid.left, Way::Across)
        .unwrap();
}

#[test]
fn beneath_a_prompt_the_window_it_acts_on_stays() {
    let mut laid = laid(Way::Across);
    laid.editor.select_window(laid.right);
    laid.editor.ask("Exit?", "keyboard-quit").unwrap();
    assert!(laid.editor.prompt().is_some());
    assert!(show_laid(&mut laid.editor, &mut laid.kept, laid.second).0);
    assert!(laid.editor.window_by_id(laid.right).is_some());
}
