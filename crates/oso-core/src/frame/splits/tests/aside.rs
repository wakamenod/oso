//! Tests of a window split at the left of an area (ADR 0122, step 6b): it takes the columns
//! asked for beside a window, a run's window or a whole pair, and gives them back to the right.
//!
//! Must not depend on: `editor`.

use super::*;

#[test]
fn a_window_split_at_the_left_takes_its_columns_first() {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 120);
    splits.split_aside(A, B, Way::Across, 40, &frame).unwrap();
    let frame = lay(&splits, 41, 120);
    assert_eq!(splits.iter().collect::<Vec<_>>(), [B, A]);
    assert_eq!(outer(&frame, B), rect(0, 0, 40, 40));
    assert!(frame.regions(B).unwrap().divider.is_some());
    assert_eq!(outer(&frame, A), rect(0, 40, 40, 80));
    // Gone, its columns go back to the window at its right.
    assert!(splits.remove_toward_next(B, &frame));
    let frame = lay(&splits, 41, 120);
    assert_eq!(outer(&frame, A), rect(0, 0, 40, 120));
}

#[test]
fn in_a_run_across_it_goes_before_its_window_and_gives_back_to_it() {
    let (mut splits, frame) = two(Way::Across);
    // A has 41 columns, B 40.
    splits.split_aside(B, C, Way::Across, 20, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    assert_eq!(splits.iter().collect::<Vec<_>>(), [A, C, B]);
    assert_eq!(outer(&frame, A), rect(0, 0, 40, 41));
    assert_eq!(outer(&frame, C), rect(0, 41, 40, 20));
    assert_eq!(outer(&frame, B), rect(0, 61, 40, 20));
    assert!(splits.remove_toward_next(C, &frame));
    let frame = lay(&splits, 41, 81);
    assert_eq!(outer(&frame, A), rect(0, 0, 40, 41), "A keeps its columns");
    assert_eq!(outer(&frame, B), rect(0, 41, 40, 40));
}

#[test]
fn beside_a_pair_it_goes_at_the_left_of_the_whole_pair() {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 160);
    splits.pair(A, Way::Across, B, &frame).unwrap();
    let frame = lay(&splits, 41, 160);
    splits.split_aside(B, C, Way::Across, 40, &frame).unwrap();
    let frame = lay(&splits, 41, 160);
    assert_eq!(splits.iter().collect::<Vec<_>>(), [C, A, B]);
    assert_eq!(outer(&frame, C), rect(0, 0, 40, 40));
    // The pair shares the rest.
    assert_eq!(outer(&frame, A), rect(0, 40, 40, 60));
    assert_eq!(outer(&frame, B), rect(0, 100, 40, 60));
}

#[test]
fn beside_a_window_of_a_run_down_it_takes_that_window_s_rows_only() {
    let (mut splits, frame) = two(Way::Down);
    splits.split_aside(B, C, Way::Across, 30, &frame).unwrap();
    let frame = lay(&splits, 41, 81);
    assert_eq!(outer(&frame, A), rect(0, 0, 20, 81));
    assert_eq!(outer(&frame, C), rect(20, 0, 20, 30));
    assert_eq!(outer(&frame, B), rect(20, 30, 20, 51));
}

#[test]
fn it_leaves_the_area_its_smallest_and_refuses_with_no_room() {
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 30);
    // 30 columns: the area keeps 10, the new window takes the other 20.
    splits.split_aside(A, B, Way::Across, 40, &frame).unwrap();
    let frame = lay(&splits, 41, 30);
    assert_eq!(outer(&frame, B).columns, 20);
    let mut splits = Splits::new(A);
    let frame = lay(&splits, 41, 20);
    assert_eq!(
        splits.split_aside(A, B, Way::Across, 40, &frame),
        Err(TooSmall)
    );
    assert_eq!(
        splits.split_aside(A, A, Way::Across, 5, &frame),
        Err(TooSmall),
        "a window already there"
    );
}
