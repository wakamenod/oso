//! Tests of the window tree and its layout (ADR 0112).
//!
//! Must not depend on: `editor`; the tree is tested with plain ids and sizes.

use super::*;
use crate::frame::{Hit, Part, layout};
use proptest::prelude::*;

const A: WindowId = WindowId::new(0);
const B: WindowId = WindowId::new(1);
const C: WindowId = WindowId::new(2);
const D: WindowId = WindowId::new(3);

fn size(rows: usize, columns: usize) -> FrameSize {
    FrameSize { rows, columns }
}

fn lay(splits: &Splits, rows: usize, columns: usize) -> FrameLayout {
    layout(size(rows, columns), splits, A, |_| 0)
}

fn rect(row: usize, column: usize, rows: usize, columns: usize) -> CellRect {
    CellRect {
        row,
        column,
        rows,
        columns,
    }
}

fn outer(frame: &FrameLayout, window: WindowId) -> CellRect {
    frame
        .regions(window)
        .map(WindowRegions::outer)
        .unwrap_or_default()
}

/// `A` split `way` into `A` and `B` in a frame of 41 rows (40 tiled) by 81 columns.
fn two(way: Way) -> (Splits, FrameLayout) {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 81);
    splits.split(A, way, B, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    (splits, frame)
}

#[test]
fn a_split_below_halves_the_rows_the_new_window_taking_the_smaller_half() {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 42, 80);
    splits.split(A, Way::Down, B, &frame).unwrap();
    let frame = lay(&splits, 42, 80);
    // 41 tiled rows: 21 and 20, each with its modeline.
    assert_eq!(outer(&frame, A), rect(0, 0, 21, 80));
    assert_eq!(outer(&frame, B), rect(21, 0, 20, 80));
    let a = frame.regions(A).unwrap();
    assert_eq!((a.text.rows, a.modeline.row), (20, 20));
    assert_eq!(a.divider, None);
    assert_eq!(frame.echo, rect(41, 0, 1, 80));
    assert_eq!(splits.iter().collect::<Vec<_>>(), [A, B]);
}

#[test]
fn a_split_right_gives_the_left_window_a_divider() {
    let (_, frame) = two(Way::Across);
    // 81 columns: 41 and 40, the left one's last column its divider.
    let a = frame.regions(A).unwrap();
    assert_eq!(outer(&frame, A), rect(0, 0, 40, 41));
    assert_eq!(a.divider, Some(rect(0, 40, 40, 1)));
    assert_eq!((a.text.columns, a.modeline.columns), (40, 40));
    let b = frame.regions(B).unwrap();
    assert_eq!(b.divider, None);
    assert_eq!(b.text, rect(0, 41, 39, 40));
}

#[test]
fn a_split_leaves_each_half_two_rows_and_ten_columns_of_text() {
    let mut splits = Splits::new(A);
    // 6 tiled rows: 3 and 3, two of text each.
    let frame = lay(&splits, 7, 80);
    assert_eq!(splits.split(A, Way::Down, B, &frame), Ok(()));
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 6, 80);
    assert_eq!(splits.split(A, Way::Down, B, &frame), Err(TooSmall));
    // 21 columns: 11 with the divider, 10.
    let frame = lay(&splits, 30, 21);
    assert_eq!(splits.split(A, Way::Across, B, &frame), Ok(()));
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 30, 20);
    assert_eq!(splits.split(A, Way::Across, B, &frame), Err(TooSmall));
    // A gutter of 4 takes from each half: 15 with the divider, 14.
    let mut splits = Splits::new(A);
    let frame = layout(size(30, 28), &splits, A, |_| 4);
    assert_eq!(splits.split(A, Way::Across, B, &frame), Err(TooSmall));
    let frame = layout(size(30, 29), &splits, A, |_| 4);
    assert_eq!(splits.split(A, Way::Across, B, &frame), Ok(()));
}

#[test]
fn a_split_refuses_a_window_already_in_the_tree_or_not_in_it() {
    let (mut splits, frame) = two(Way::Down);
    assert_eq!(splits.split(A, Way::Down, B, &frame), Err(TooSmall));
    assert_eq!(splits.split(C, Way::Down, D, &frame), Err(TooSmall));
}

#[test]
fn a_split_the_way_of_its_run_adds_a_sibling_and_moves_no_other_window() {
    let (mut splits, frame) = two(Way::Down);
    let before = outer(&frame, B);
    splits.split(A, Way::Down, C, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    assert_eq!(outer(&frame, B), before);
    assert_eq!(outer(&frame, A), rect(0, 0, 10, 81));
    assert_eq!(outer(&frame, C), rect(10, 0, 10, 81));
    assert_eq!(splits.iter().collect::<Vec<_>>(), [A, C, B]);
    // One run of three, not a run inside a run.
    splits.balance();
    let frame = lay(&splits, 41, 81);
    let rows: Vec<usize> = [A, C, B].iter().map(|w| outer(&frame, *w).rows).collect();
    assert_eq!(rows, [14, 13, 13]);
}

#[test]
fn a_split_the_other_way_makes_a_run_in_the_window_place() {
    let (mut splits, frame) = two(Way::Down);
    splits.split(B, Way::Across, C, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    assert_eq!(outer(&frame, A), rect(0, 0, 20, 81));
    assert_eq!(outer(&frame, B), rect(20, 0, 20, 41));
    assert_eq!(outer(&frame, C), rect(20, 41, 20, 40));
    assert_eq!(splits.iter().collect::<Vec<_>>(), [A, B, C]);
}

#[test]
fn a_removed_window_gives_its_cells_to_the_one_before_it_or_after_the_first() {
    let (mut splits, frame) = two(Way::Down);
    splits.split(B, Way::Down, C, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    let (a, b) = (outer(&frame, A), outer(&frame, B));
    assert!(splits.remove(C, &frame));
    let after = lay(&splits, 41, 81);
    assert_eq!(outer(&after, A), a);
    assert_eq!(outer(&after, B).rows, b.rows + outer(&frame, C).rows);
    assert!(splits.remove(A, &after));
    assert_eq!(splits, Splits::new(B));
    assert!(
        !splits.remove(B, &lay(&splits, 41, 81)),
        "the only window stays"
    );
}

#[test]
fn a_run_left_with_one_child_goes_and_one_of_the_same_way_merges() {
    // A | (B over (C | D)): removing B leaves C | D, which joins A's run.
    let (mut splits, frame) = two(Way::Across);
    splits.split(B, Way::Down, C, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    splits.split(C, Way::Across, D, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    let a = outer(&frame, A);
    assert!(splits.remove(B, &frame));
    let mut merged = Splits::new(A);
    let one = lay(&merged, 41, 81);
    merged.split(A, Way::Across, C, &one).unwrap();
    let one = lay(&merged, 41, 81);
    merged.split(C, Way::Across, D, &one).unwrap();
    assert_eq!(splits.iter().collect::<Vec<_>>(), [A, C, D]);
    let frame = lay(&splits, 41, 81);
    assert_eq!(outer(&frame, A), a, "A keeps its cells");
    assert_eq!(outer(&frame, C).rows, 40);
    // A run of three, as if split so: balancing gives thirds.
    splits.balance();
    let frame = lay(&splits, 41, 81);
    let columns: Vec<usize> = [A, C, D]
        .iter()
        .map(|w| outer(&frame, *w).columns)
        .collect();
    assert_eq!(columns, [27, 27, 27]);
}

#[test]
fn a_resize_grows_into_the_windows_after_then_before_and_shrinks_into_the_next() {
    let (mut splits, frame) = two(Way::Down);
    splits.split(B, Way::Down, C, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    // A 20, B 10, C 10.
    assert_eq!(splits.resize(B, Way::Down, 5, &frame), 5);
    let frame = lay(&splits, 41, 81);
    assert_eq!(
        [A, B, C].map(|w| outer(&frame, w).rows),
        [20, 15, 5],
        "from C first"
    );
    // C keeps 3; the rest comes from A.
    assert_eq!(splits.resize(B, Way::Down, 4, &frame), 4);
    let frame = lay(&splits, 41, 81);
    assert_eq!([A, B, C].map(|w| outer(&frame, w).rows), [18, 19, 3]);
    assert_eq!(
        splits.resize(C, Way::Down, -1, &frame),
        0,
        "C is at its least"
    );
    assert_eq!(splits.resize(C, Way::Down, -9, &frame), 0);
    assert_eq!(splits.resize(B, Way::Down, -2, &frame), -2);
    let frame = lay(&splits, 41, 81);
    assert_eq!(
        [A, B, C].map(|w| outer(&frame, w).rows),
        [18, 17, 5],
        "into C"
    );
    assert_eq!(splits.resize(A, Way::Down, 100, &frame), 16);
    assert_eq!(
        splits.resize(A, Way::Across, 1, &lay(&splits, 41, 81)),
        0,
        "no run across"
    );
}

#[test]
fn a_frame_resized_keeps_the_proportions_and_back_gives_the_same_cells() {
    let (mut splits, frame) = two(Way::Across);
    splits.resize(A, Way::Across, 10, &frame);
    let before = lay(&splits, 41, 81);
    let half = lay(&splits, 41, 40);
    assert_eq!(outer(&half, A).columns, 25);
    for columns in [200, 33, 81] {
        let _ = lay(&splits, 41, columns);
    }
    assert_eq!(lay(&splits, 41, 81), before);
}

#[test]
fn a_frame_too_small_for_every_window_drops_the_last_but_never_the_kept() {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 81);
    splits.split(A, Way::Down, B, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    splits.split(B, Way::Down, C, &frame).unwrap();
    // 4 tiled rows hold two windows of a row and a modeline.
    let small = lay(&splits, 5, 81);
    assert_eq!([A, B, C].map(|w| outer(&small, w).rows), [2, 2, 0]);
    let kept = layout(size(5, 81), &splits, C, |_| 0);
    assert_eq!([A, B, C].map(|w| outer(&kept, w).rows), [2, 0, 2]);
    // One row: the kept window alone, with no modeline.
    let one = layout(size(2, 81), &splits, B, |_| 0);
    assert_eq!([A, B, C].map(|w| outer(&one, w).rows), [0, 1, 0]);
    assert_eq!(one.regions(B).unwrap().modeline.rows, 0);
    // And they come back.
    assert_eq!(lay(&splits, 41, 81), lay(&splits, 41, 81));
    assert_eq!(outer(&lay(&splits, 41, 81), C).rows, 10);
}

#[test]
fn a_cell_is_in_a_window_part_and_a_window_has_neighbours() {
    // A | (B over C)
    let (mut splits, frame) = two(Way::Across);
    splits.split(B, Way::Down, C, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    let hit = |row, column| frame.hit(row, column);
    assert_eq!(
        hit(0, 0),
        Some(Hit {
            window: A,
            part: Part::Text
        })
    );
    assert_eq!(hit(39, 0).map(|h| h.part), Some(Part::Modeline));
    assert_eq!(hit(5, 40).map(|h| h.part), Some(Part::Divider));
    assert_eq!(hit(5, 41).map(|h| h.window), Some(B));
    assert_eq!(hit(25, 41).map(|h| h.window), Some(C));
    assert_eq!(hit(40, 0), None, "the echo area");
    assert_eq!(frame.neighbour(A, Side::Right, 5), Some(B));
    assert_eq!(frame.neighbour(A, Side::Right, 25), Some(C));
    assert_eq!(frame.neighbour(C, Side::Left, 25), Some(A));
    assert_eq!(frame.neighbour(C, Side::Up, 50), Some(B));
    assert_eq!(frame.neighbour(B, Side::Down, 50), Some(C));
    assert_eq!(frame.neighbour(A, Side::Left, 5), None);
    assert_eq!(frame.neighbour(B, Side::Up, 50), None);
}

#[test]
fn floats_go_over_the_tiled_area_but_its_bottom_modelines() {
    let (_, frame) = two(Way::Down);
    assert_eq!(frame.tiled(), rect(0, 0, 39, 81));
    let (_, frame) = two(Way::Across);
    assert_eq!(frame.tiled(), rect(0, 0, 39, 81));
}

#[test]
fn shares_go_by_the_largest_remainder() {
    assert_eq!(largest_remainder(10, [1, 1, 1].into_iter()), [4, 3, 3]);
    assert_eq!(largest_remainder(10, [1, 2].into_iter()), [3, 7]);
    assert_eq!(largest_remainder(0, [1, 2].into_iter()), [0, 0]);
    assert_eq!(
        largest_remainder(usize::MAX, [1, 1].into_iter())
            .iter()
            .sum::<usize>(),
        usize::MAX
    );
}

/// What the person does to the tree.
#[derive(Clone, Debug)]
enum Op {
    Split(usize, bool),
    Remove(usize),
    Resize(usize, bool, isize),
    Balance,
    Frame(usize, usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..8, any::<bool>()).prop_map(|(w, across)| Op::Split(w, across)),
        (0usize..8).prop_map(Op::Remove),
        (0usize..8, any::<bool>(), -30isize..30).prop_map(|(w, a, c)| Op::Resize(w, a, c)),
        Just(Op::Balance),
        (0usize..80, 0usize..200).prop_map(|(r, c)| Op::Frame(r, c)),
    ]
}

proptest! {
    /// Whatever is done, the windows' cells add up to the tiled area, never overlap, show
    /// each window of the tree once in its order, and give each its least while the frame
    /// holds every window so.
    #[test]
    fn prop_the_layout_tiles_the_frame(ops in prop::collection::vec(op(), 0..40), gutter in 0usize..6) {
        let mut splits = Splits::new(A);
        let mut next = 1;
        let (mut rows, mut columns) = (41, 120);
        for op in ops {
            let windows: Vec<WindowId> = splits.iter().collect();
            let frame = layout(size(rows, columns), &splits, windows[0], |_| gutter);
            let pick = |i: usize| windows[i % windows.len()];
            match op {
                Op::Split(w, across) => {
                    let way = if across { Way::Across } else { Way::Down };
                    if splits.split(pick(w), way, WindowId::new(next), &frame).is_ok() {
                        next += 1;
                    }
                }
                Op::Remove(w) => {
                    let removed = splits.remove(pick(w), &frame);
                    prop_assert_eq!(removed, windows.len() > 1);
                }
                Op::Resize(w, across, cells) => {
                    let way = if across { Way::Across } else { Way::Down };
                    let moved = splits.resize(pick(w), way, cells, &frame);
                    prop_assert!(moved.unsigned_abs() <= cells.unsigned_abs());
                }
                Op::Balance => splits.balance(),
                Op::Frame(r, c) => (rows, columns) = (r, c),
            }
            let windows: Vec<WindowId> = splits.iter().collect();
            let keep = windows[windows.len() / 2];
            let frame = layout(size(rows, columns), &splits, keep, |_| gutter);
            let laid: Vec<WindowId> = frame.windows().iter().map(|(id, _)| *id).collect();
            prop_assert_eq!(&laid, &windows);
            let tiled = frame.echo.row;
            let mut covered = 0;
            for (i, (id, regions)) in frame.windows().iter().enumerate() {
                let o = regions.outer();
                covered += o.rows * o.columns;
                prop_assert!(o.row + o.rows <= tiled && o.column + o.columns <= columns);
                if *id == keep && tiled > 0 && columns > 0 {
                    prop_assert!(o.rows > 0 && o.columns > 0, "the kept window shows");
                }
                for (_, other) in &frame.windows()[i + 1..] {
                    let p = other.outer();
                    let apart = o.row + o.rows <= p.row || p.row + p.rows <= o.row
                        || o.column + o.columns <= p.column || p.column + p.columns <= o.column;
                    prop_assert!(apart || o.rows * o.columns == 0 || p.rows * p.columns == 0);
                }
                prop_assert_eq!(regions.gutter.columns + regions.text.columns
                    + regions.divider.map_or(0, |d| d.columns), o.columns);
            }
            let all_shown = frame.windows().iter().all(|(_, r)| {
                let o = r.outer();
                o.rows > 0 && o.columns > 0
            });
            // A frame of one tiled row has no room for a modeline under any window.
            if all_shown && tiled >= 2 {
                prop_assert_eq!(covered, tiled * columns, "the windows fill the tiled area");
                for (_, regions) in frame.windows() {
                    if frame.windows().len() > 1 {
                        prop_assert!(regions.text.rows >= 1 && regions.modeline.rows == 1);
                    }
                }
            }
        }
    }
}
