//! Tests of the window tree and its layout (ADR 0112).
//!
//! Must not depend on: `editor`; the tree is tested with plain ids and sizes.

use super::cells::largest_remainder;
use super::*;
use crate::frame::{FrameSize, Hit, Part, layout};
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
        .unwrap_or(CellRect::EMPTY)
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
    // With line numbers on, the narrowest gutter, 4, takes from each half: 15 with the
    // divider, 14. The window's own gutter, wider for a long buffer, does not count.
    for own in [4, 7] {
        let mut splits = Splits::new(A);
        splits.set_gutter(4);
        let frame = layout(size(30, 28), &splits, A, |_| own);
        assert_eq!(splits.split(A, Way::Across, B, &frame), Err(TooSmall));
        let frame = layout(size(30, 29), &splits, A, |_| own);
        assert_eq!(splits.split(A, Way::Across, B, &frame), Ok(()), "{own}");
    }
}

#[test]
fn a_wider_gutter_than_the_narrowest_leaves_fewer_text_columns_and_moves_no_floor() {
    // Resized to the floor, a window of a 100,000-line buffer keeps 7 text columns beside
    // its 7, where one of 3 lines keeps 10 beside its 4: the same cells either way.
    let floor = |own: usize| {
        let mut splits = Splits::new(A);
        splits.set_gutter(4);
        let frame = layout(size(30, 61), &splits, A, |_| own);
        splits.split(A, Way::Across, B, &frame).unwrap();
        let frame = layout(size(30, 61), &splits, A, |_| own);
        splits.resize(A, Way::Across, isize::MAX, &frame);
        let frame = layout(size(30, 61), &splits, A, |_| own);
        *frame.regions(B).unwrap()
    };
    let (few, many) = (floor(4), floor(7));
    assert_eq!(
        few.outer(),
        many.outer(),
        "the stop says nothing of the line count"
    );
    assert_eq!((few.gutter.columns, few.text.columns), (4, 10));
    assert_eq!((many.gutter.columns, many.text.columns), (7, 7));
}

#[test]
fn a_split_leaves_each_half_room_across_the_split_too() {
    // Stacked halves of 9 columns of text, or with a gutter of 4 and 13 columns.
    for (rows, columns, gutter) in [(41, 9, 0), (41, 13, 4), (0, 31, 0)] {
        let mut splits = Splits::new(A);
        splits.set_gutter(gutter);
        let frame = layout(size(rows, columns), &splits, A, |_| gutter);
        assert_eq!(
            splits.split(A, Way::Down, B, &frame),
            Err(TooSmall),
            "{rows} by {columns}"
        );
    }
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 10);
    assert_eq!(splits.split(A, Way::Down, B, &frame), Ok(()));
    // Side by side halves of 1 row of text, or none.
    for rows in [0, 1, 3] {
        let mut splits = Splits::new(A);
        let frame = lay(&splits, rows, 31);
        assert_eq!(
            splits.split(A, Way::Across, B, &frame),
            Err(TooSmall),
            "{rows} rows"
        );
    }
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 4, 31);
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
    assert_eq!(outer(&lay(&splits, 41, 81), C).rows, 10);
}

#[test]
fn a_run_too_small_for_its_windows_still_gives_its_cells_to_one() {
    // A over D over (B | C), in one column: B takes the cells of B and C.
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 81);
    splits.split(A, Way::Down, D, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    splits.split(D, Way::Down, B, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    splits.split(B, Way::Across, C, &frame).unwrap();
    let thin = lay(&splits, 7, 1);
    assert_eq!(outer(&thin, B), rect(4, 0, 2, 1));
    assert_eq!(outer(&thin, C).columns, 0);
    for row in 0..6 {
        assert!(thin.hit(row, 0).is_some(), "row {row} is a window's");
    }
}

#[test]
fn a_window_whose_right_neighbour_has_no_room_has_no_divider() {
    // A | B | C at 23 columns: room for A and B, each with a column of text and a divider,
    // but not C; B is the right-most shown and draws none.
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 81);
    splits.split(A, Way::Across, B, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    splits.split(B, Way::Across, C, &frame).unwrap();
    let narrow = lay(&splits, 41, 4);
    assert_eq!(outer(&narrow, C).columns, 0);
    assert!(narrow.regions(A).unwrap().divider.is_some());
    assert_eq!(narrow.regions(B).unwrap().divider, None);
    assert_eq!(outer(&narrow, A).columns + outer(&narrow, B).columns, 4);
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
    Pair(usize, bool),
    Turn(usize, bool),
    Break(usize),
    Unpair(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..8, any::<bool>()).prop_map(|(w, across)| Op::Split(w, across)),
        (0usize..8).prop_map(Op::Remove),
        (0usize..8, any::<bool>(), -300isize..300).prop_map(|(w, a, c)| Op::Resize(w, a, c)),
        Just(Op::Balance),
        (0usize..40, 0usize..100).prop_map(|(r, c)| Op::Frame(r, c)),
        (0usize..8, any::<bool>()).prop_map(|(w, across)| Op::Pair(w, across)),
        (0usize..8, any::<bool>()).prop_map(|(w, across)| Op::Turn(w, across)),
        (0usize..8).prop_map(Op::Break),
        (0usize..8).prop_map(Op::Unpair),
    ]
}

proptest! {
    /// Whatever is done, every cell of the tiled area is one window's and no two windows
    /// overlap; the layout shows each window of the tree once in its order, keeps the kept
    /// one shown, gives each shown window a column and a row of text, and a row and a
    /// modeline while all fit. `hit` and `neighbour` agree with the cells, a frame resized
    /// and back lays out the same, and a removal shrinks none of the windows left but by a
    /// divider it no longer draws. A pair's windows are shown together or not at all, and
    /// share its cells (ADR 0122).
    #[test]
    fn prop_the_layout_tiles_the_frame(ops in prop::collection::vec(op(), 0..40), gutter in 0usize..6) {
        let mut splits = Splits::new(A);
        let mut next = 1;
        let (mut rows, mut columns) = (31, 90);
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
                    // The only window or pair stays.
                    let alone = splits.only(pick(w)).iter().count() == windows.len();
                    let removed = splits.remove(pick(w), &frame);
                    prop_assert_eq!(removed, !alone);
                    let after = layout(size(rows, columns), &splits, windows[0], |_| gutter);
                    let all_shown = frame.windows().iter().all(|(_, r)| shown(r.outer()));
                    if removed && all_shown && pick(w) != windows[0] {
                        // The columns beside its divider: a window held at its smallest keeps
                        // a column for its divider, which it no longer draws once the removal
                        // leaves it at the right edge, where its weight lays it out again.
                        let beside = |r: &WindowRegions| {
                            r.outer().columns - r.divider.map_or(0, |d| d.columns)
                        };
                        for (id, regions) in after.windows() {
                            let Some(before) = frame.regions(*id) else { continue };
                            let (old, new) = (before.outer(), regions.outer());
                            // A run that takes the cells grows its windows in proportion,
                            // so they may move, but none gets smaller.
                            prop_assert!(
                                new.rows >= old.rows && beside(regions) >= beside(before),
                                "{:?} shrank from {:?} to {:?}", id, old, new
                            );
                        }
                    }
                }
                Op::Resize(w, across, cells) => {
                    let way = if across { Way::Across } else { Way::Down };
                    let moved = splits.resize(pick(w), way, cells, &frame);
                    prop_assert!(moved.unsigned_abs() <= cells.unsigned_abs());
                    prop_assert!(moved == 0 || moved.signum() == cells.signum());
                }
                Op::Balance => splits.balance(),
                Op::Frame(r, c) => (rows, columns) = (r, c),
                Op::Pair(w, across) => {
                    let way = if across { Way::Across } else { Way::Down };
                    if splits.pair(pick(w), way, WindowId::new(next), &frame).is_ok() {
                        next += 1;
                    }
                }
                Op::Turn(w, across) => {
                    let way = if across { Way::Across } else { Way::Down };
                    let _ = splits.turn(pick(w), way, &frame);
                }
                Op::Break(w) => {
                    splits.break_pair(pick(w), &frame);
                }
                Op::Unpair(w) => {
                    splits.unpair(pick(w));
                }
            }
            let windows: Vec<WindowId> = splits.iter().collect();
            let keep = windows[windows.len() / 2];
            let frame = layout(size(rows, columns), &splits, keep, |_| gutter);
            let laid: Vec<WindowId> = frame.windows().iter().map(|(id, _)| *id).collect();
            prop_assert_eq!(&laid, &windows);
            let resized = layout(size(rows / 2 + 3, columns * 2 + 1), &splits, keep, |_| gutter);
            prop_assert!(resized.windows().len() == frame.windows().len());
            prop_assert_eq!(&layout(size(rows, columns), &splits, keep, |_| gutter), &frame);
            let tiled = frame.echo.row;
            // Each cell of the tiled area is in exactly one window, as `hit` says.
            for row in 0..tiled {
                for column in 0..columns {
                    let owners: Vec<WindowId> = frame
                        .windows()
                        .iter()
                        .filter(|(_, r)| r.outer().contains(row, column))
                        .map(|(id, _)| *id)
                        .collect();
                    prop_assert_eq!(owners.len(), 1, "cell {},{}", row, column);
                    prop_assert_eq!(frame.hit(row, column).map(|h| h.window), Some(owners[0]));
                }
            }
            prop_assert_eq!(frame.hit(tiled, 0), None);
            let every_shown = frame.windows().iter().all(|(_, r)| shown(r.outer()));
            for [a, b] in splits.pairs() {
                let (a_cells, b_cells) = (outer(&frame, a), outer(&frame, b));
                // Together, but in an area too small for both, where the first keeps it.
                let lone = if shown(a_cells) { a_cells } else { b_cells };
                let too_small = match splits.pair_way(a) {
                    Some(Way::Down) => lone.rows < 4,
                    _ => lone.columns < 4,
                };
                if keep != a && keep != b && !too_small {
                    prop_assert_eq!(shown(a_cells), shown(b_cells), "a pair shows together");
                }
                if every_shown {
                    let across = a_cells.row == b_cells.row && a_cells.rows == b_cells.rows
                        && a_cells.columns.abs_diff(b_cells.columns) <= 1;
                    let down = a_cells.column == b_cells.column && a_cells.columns == b_cells.columns
                        && a_cells.rows.abs_diff(b_cells.rows) <= 1;
                    prop_assert!(across || down, "a pair shares its cells: {:?} {:?}", a_cells, b_cells);
                }
            }
            let multiple = frame.windows().iter().filter(|(_, r)| shown(r.outer())).count() > 1;
            for (id, regions) in frame.windows() {
                let o = regions.outer();
                prop_assert_eq!(regions.gutter.columns + regions.text.columns
                    + regions.divider.map_or(0, |d| d.columns), o.columns);
                if *id == keep && tiled > 0 && columns > 0 {
                    prop_assert!(shown(o), "the kept window shows");
                }
                if !shown(o) {
                    continue;
                }
                prop_assert!(regions.text.columns >= 1 && regions.text.rows >= 1);
                if let Some(divider) = regions.divider {
                    prop_assert!(divider.column + 1 < columns, "a divider has a window to its right");
                }
                if multiple && tiled >= 2 {
                    prop_assert_eq!(regions.modeline.rows, 1);
                }
                // The window past each edge, as `hit` finds it, is the neighbour there.
                for side in [Side::Left, Side::Right, Side::Up, Side::Down] {
                    let cells: Vec<usize> = match side {
                        Side::Left | Side::Right => (o.row..o.row + o.rows).collect(),
                        Side::Up | Side::Down => (o.column..o.column + o.columns).collect(),
                    };
                    for cell in cells {
                        let past = match side {
                            Side::Left => o.column.checked_sub(1).map(|c| (cell, c)),
                            Side::Right => Some((cell, o.column + o.columns)),
                            Side::Up => o.row.checked_sub(1).map(|r| (r, cell)),
                            Side::Down => Some((o.row + o.rows, cell)),
                        };
                        let there = past.and_then(|(r, c)| frame.hit(r, c)).map(|h| h.window);
                        prop_assert_eq!(frame.neighbour(*id, side, cell), there);
                    }
                }
            }
        }
    }
}

fn shown(cells: CellRect) -> bool {
    cells.rows > 0 && cells.columns > 0
}

mod removal;
