//! Tests of removing a window from the tree (ADR 0112).
//!
//! Must not depend on: `editor`; the tree is tested with plain ids and sizes.

use super::*;

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
fn a_window_held_at_its_smallest_loses_only_its_divider_when_the_window_right_of_it_goes() {
    // (A over ((C over E) | D)) | B, the pair C over E made 11 columns wider at 31 by 90, then
    // the frame 5 columns wide: D's share of the 4 left of B is under 1 column, so D is held
    // at its smallest, a column of text and its divider.
    let e = WindowId::new(4);
    let (mut splits, frame) = (Splits::new(A), lay(&Splits::new(A), 31, 90));
    splits.split(A, Way::Across, B, &frame).unwrap();
    splits
        .split(A, Way::Down, C, &lay(&splits, 31, 90))
        .unwrap();
    splits
        .split(C, Way::Across, D, &lay(&splits, 31, 90))
        .unwrap();
    splits.pair(C, Way::Down, e, &lay(&splits, 31, 90)).unwrap();
    assert_eq!(splits.resize(C, Way::Across, 11, &lay(&splits, 31, 90)), 11);
    let frame = lay(&splits, 7, 5);
    assert_eq!(outer(&frame, D), rect(2, 2, 4, 2));
    assert!(frame.regions(D).unwrap().divider.is_some());
    // B goes and its column goes to the run left of it, which lays D out by its weight again:
    // a column of text at the right edge, with no divider to keep.
    assert!(splits.remove(B, &frame));
    let after = lay(&splits, 7, 5);
    assert_eq!(outer(&after, D), rect(2, 4, 4, 1));
    assert_eq!(after.regions(D).unwrap().divider, None);
    assert_eq!(after.regions(D).unwrap().text.columns, 1);
    for window in [A, C, e] {
        let (old, new) = (outer(&frame, window), outer(&after, window));
        assert!(
            new.rows >= old.rows && new.columns > old.columns,
            "{window:?} grows"
        );
    }
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
