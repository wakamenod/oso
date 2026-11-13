//! Tests of a pair of windows in the tree (ADR 0122, step 10).
//!
//! Must not depend on: `editor`; the tree is tested with plain ids and sizes.

use super::super::{CellRect, Splits, TooSmall, Way, WindowRegions};
use crate::frame::{FrameLayout, FrameSize, Tiling, layout};
use crate::window::WindowId;

const A: WindowId = WindowId::new(0);
const B: WindowId = WindowId::new(1);
const C: WindowId = WindowId::new(2);
const D: WindowId = WindowId::new(3);

fn lay(splits: &Splits, rows: usize, columns: usize) -> FrameLayout {
    layout(FrameSize { rows, columns }, splits, A, |_| 0)
}

fn outer(frame: &FrameLayout, window: WindowId) -> CellRect {
    frame
        .regions(window)
        .map(WindowRegions::outer)
        .unwrap_or(CellRect::EMPTY)
}

/// `A` paired `way` with `B` in a frame of 41 rows (40 tiled) by 81 columns.
fn paired(way: Way) -> Splits {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 81);
    splits.pair(A, way, B, &frame).unwrap();
    splits
}

/// `C` split off `at` `way` in the frame of 41 by 81.
fn split(splits: &mut Splits, at: WindowId, way: Way, new: WindowId) {
    let frame = lay(splits, 41, 81);
    splits.split(at, way, new, &frame).unwrap();
}

#[test]
fn a_pair_shares_its_windows_cells_and_names_each_other() {
    let splits = paired(Way::Across);
    let frame = lay(&splits, 41, 81);
    assert_eq!(
        (outer(&frame, A).columns, outer(&frame, B).columns),
        (41, 40)
    );
    assert_eq!(splits.pairs(), [[A, B]]);
    assert_eq!((splits.pair_of(A), splits.pair_of(B)), (Some(B), Some(A)));
    let mut splits = paired(Way::Down);
    assert_eq!(splits.pair_of(C), None);
    let frame = lay(&splits, 41, 81);
    assert_eq!(
        splits.pair(A, Way::Across, C, &frame),
        Err(TooSmall),
        "a window pairs once"
    );
    let frame = lay(&splits, 41, 81);
    assert_eq!((outer(&frame, A).rows, outer(&frame, B).rows), (20, 20));
}

#[test]
fn a_pair_is_refused_where_a_split_would_be() {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 30, 20);
    assert_eq!(splits.pair(A, Way::Across, B, &frame), Err(TooSmall));
    let frame = lay(&splits, 6, 80);
    assert_eq!(splits.pair(A, Way::Down, B, &frame), Err(TooSmall));
    assert!(splits.pairs().is_empty());
}

#[test]
fn a_pair_side_by_side_stays_one_child_of_a_run_side_by_side() {
    // A beside C, then A paired with B: the pair is one child, half the frame.
    let mut splits = Splits::new(A);
    split(&mut splits, A, Way::Across, C);
    let frame = lay(&splits, 41, 81);
    splits.pair(A, Way::Across, B, &frame).unwrap();
    assert_eq!(splits.iter().collect::<Vec<_>>(), [A, B, C]);
    let frame = lay(&splits, 41, 81);
    let (a, b, c) = (outer(&frame, A), outer(&frame, B), outer(&frame, C));
    assert_eq!((a.columns, b.columns, c.columns), (21, 20, 40));
    let Tiling::Run { children, .. } = splits.tiling() else {
        panic!("a run");
    };
    assert_eq!(children.len(), 2, "not merged: {children:?}");
}

#[test]
fn splitting_a_window_of_a_pair_splits_the_pairs_area() {
    for way in [Way::Across, Way::Down] {
        let mut splits = paired(Way::Across);
        split(&mut splits, B, way, C);
        assert_eq!(splits.pairs(), [[A, B]], "the pair holds");
        let frame = lay(&splits, 41, 81);
        let (a, b, c) = (outer(&frame, A), outer(&frame, B), outer(&frame, C));
        match way {
            // Below the whole pair, across all of it.
            Way::Down => {
                assert_eq!((c.row, c.column, c.columns), (a.rows, 0, 81));
                assert_eq!(a.columns + b.columns, 81);
            }
            // At the right of the whole pair.
            Way::Across => {
                assert_eq!(c.column, a.columns + b.columns);
                assert_eq!((a.rows, b.rows, c.rows), (40, 40, 40));
            }
        }
    }
}

#[test]
fn deleting_a_window_of_a_pair_deletes_both_and_a_pair_alone_stays() {
    let mut splits = paired(Way::Across);
    let frame = lay(&splits, 41, 81);
    assert!(
        !splits.remove(B, &frame),
        "the pair is the frame's only one"
    );
    split(&mut splits, A, Way::Down, C);
    let frame = lay(&splits, 41, 81);
    assert!(splits.remove(B, &frame));
    assert_eq!(splits.iter().collect::<Vec<_>>(), [C]);
    assert!(splits.pairs().is_empty());
}

#[test]
fn a_pair_resizes_as_one_its_windows_kept_the_same() {
    // The pair above C: taller by 6 rows, each window as tall.
    let mut splits = paired(Way::Across);
    split(&mut splits, A, Way::Down, C);
    let frame = lay(&splits, 41, 81);
    assert_eq!(splits.resize(B, Way::Down, 6, &frame), 6);
    let frame = lay(&splits, 41, 81);
    assert_eq!((outer(&frame, A).rows, outer(&frame, B).rows), (26, 26));
    // Nothing side by side around the pair, so it cannot grow wider, nor its windows apart.
    assert_eq!(splits.resize(A, Way::Across, 4, &frame), 0);
    // The pair beside D: wider by 10, shared between its windows.
    let mut splits = paired(Way::Across);
    split(&mut splits, B, Way::Across, D);
    let frame = lay(&splits, 41, 81);
    let before = outer(&frame, A).columns + outer(&frame, B).columns;
    assert_eq!(splits.resize(A, Way::Across, 10, &frame), 10);
    let frame = lay(&splits, 41, 81);
    let (a, b) = (outer(&frame, A).columns, outer(&frame, B).columns);
    assert_eq!(a + b, before + 10);
    assert!(a.abs_diff(b) <= 1, "{a} and {b}");
    // Balanced, the pair is one child among two.
    splits.balance();
    let frame = lay(&splits, 41, 81);
    let pair = outer(&frame, A).columns + outer(&frame, B).columns;
    assert!(pair.abs_diff(outer(&frame, D).columns) <= 1);
}

#[test]
fn a_pair_turns_stacked_and_back_where_it_has_room() {
    let mut splits = paired(Way::Across);
    let frame = lay(&splits, 41, 81);
    assert_eq!(splits.turn(A, Way::Down, &frame), Ok(true));
    let frame = lay(&splits, 41, 81);
    let (a, b) = (outer(&frame, A), outer(&frame, B));
    assert_eq!((a.row, b.row, a.columns), (0, 20, 81), "A above");
    assert_eq!(
        splits.turn(B, Way::Down, &frame),
        Ok(false),
        "stacked already"
    );
    assert_eq!(splits.turn(C, Way::Across, &frame), Ok(false), "no pair");
    assert_eq!(splits.turn(B, Way::Across, &frame), Ok(true));
    // 5 tiled rows hold two windows side by side, not stacked.
    let frame = lay(&splits, 6, 81);
    assert_eq!(splits.turn(A, Way::Down, &frame), Err(TooSmall));
    // 20 columns hold two stacked, not side by side.
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 20);
    splits.pair(A, Way::Down, B, &frame).unwrap();
    let frame = lay(&splits, 41, 20);
    assert_eq!(splits.turn(A, Way::Across, &frame), Err(TooSmall));
}

#[test]
fn a_broken_pair_keeps_its_cells_and_merges_into_a_run_of_its_way() {
    let mut splits = paired(Way::Across);
    split(&mut splits, B, Way::Across, C);
    let frame = lay(&splits, 41, 81);
    assert!(splits.break_pair(A, &frame));
    assert!(!splits.break_pair(A, &frame), "broken once");
    assert!(splits.pairs().is_empty());
    let after = lay(&splits, 41, 81);
    assert_eq!(after, frame, "the same cells");
    let Tiling::Run { children, .. } = splits.tiling() else {
        panic!("a run");
    };
    assert_eq!(children.len(), 3, "merged");
    // A pair that is the frame becomes a run of two.
    let mut splits = paired(Way::Down);
    let frame = lay(&splits, 41, 81);
    assert!(splits.break_pair(B, &frame));
    assert_eq!(lay(&splits, 41, 81), frame);
}

#[test]
fn unpairing_leaves_the_kept_window_the_pairs_cells() {
    let mut splits = paired(Way::Across);
    split(&mut splits, A, Way::Down, C);
    let frame = lay(&splits, 41, 81);
    let pair_rows = outer(&frame, A).rows;
    assert_eq!(splits.unpair(B), Some(A));
    assert_eq!(splits.unpair(B), None);
    let frame = lay(&splits, 41, 81);
    assert_eq!(splits.iter().collect::<Vec<_>>(), [B, C]);
    assert_eq!(
        outer(&frame, B),
        CellRect {
            row: 0,
            column: 0,
            rows: pair_rows,
            columns: 81
        }
    );
}

#[test]
fn a_pair_with_no_room_is_hidden_whole() {
    // C, then the pair: in 4 columns, C keeps 2 and the pair, needing 3, shows not at all.
    let mut splits = Splits::new(C);
    let frame = layout(
        FrameSize {
            rows: 41,
            columns: 81,
        },
        &splits,
        C,
        |_| 0,
    );
    splits.split(C, Way::Across, A, &frame).unwrap();
    let frame = layout(
        FrameSize {
            rows: 41,
            columns: 81,
        },
        &splits,
        C,
        |_| 0,
    );
    splits.pair(A, Way::Across, B, &frame).unwrap();
    let frame = layout(
        FrameSize {
            rows: 41,
            columns: 4,
        },
        &splits,
        C,
        |_| 0,
    );
    assert_eq!(outer(&frame, C).columns, 4);
    assert_eq!((outer(&frame, A).columns, outer(&frame, B).columns), (0, 0));
}

#[test]
fn only_a_window_of_a_pair_keeps_the_pair() {
    let mut splits = paired(Way::Across);
    split(&mut splits, A, Way::Down, C);
    assert_eq!(splits.only(B).iter().collect::<Vec<_>>(), [A, B]);
    assert_eq!(splits.only(B).pairs(), [[A, B]]);
    assert_eq!(splits.only(C).iter().collect::<Vec<_>>(), [C]);
}

#[test]
fn a_restart_takes_a_pair_back_as_two_windows_of_their_own_in_their_cells() {
    // Handed over as data, the pair inside a run of its way is merged with its shares kept.
    let mut splits = paired(Way::Across);
    split(&mut splits, B, Way::Across, C);
    let frame = lay(&splits, 41, 81);
    let mut next = 10;
    let (back, ids) = Splits::from_tiling(&splits.tiling(), A, &mut next).unwrap();
    assert!(back.pairs().is_empty());
    let after = lay(&back, 41, 81);
    let columns = |frame: &FrameLayout, ids: &[WindowId]| -> Vec<usize> {
        ids.iter().map(|id| outer(frame, *id).columns).collect()
    };
    assert_eq!(columns(&after, &ids), columns(&frame, &[A, B, C]));
}
