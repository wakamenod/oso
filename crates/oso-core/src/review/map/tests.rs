//! The tests of the line map: cases ported from the author's previous review, and properties
//! checked against a reference that walks the new side line by line, as that review did,
//! where the map walks the hunks' edges.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use super::super::shape::ShapeError;
use super::super::tests::{base_hunks, blob, diff, hunk, lcs, strs};
use super::*;

fn shape(base: Base, hunks: Vec<super::super::shape::Hunk>) -> Shape {
    Shape::new(base, hunks).unwrap()
}

fn map(from: &Shape, to: &Shape, base: Option<&[BaseHunk]>) -> LineMap {
    LineMap::new(from, to, base, |a: &[&[u8]], b: &[&[u8]]| lcs(a, b))
}

fn carried(map: &LineMap, side: Side, numbers: &[u32]) -> Vec<Option<u32>> {
    numbers
        .iter()
        .map(|&number| map.carry(side, number))
        .collect()
}

#[test]
fn lines_put_in_above_move_a_line_down() {
    let base = blob("a");
    let from = shape(
        base.clone(),
        vec![hunk(
            7,
            &[],
            7,
            &["def share():", "    t = total()", "    return p / t"],
        )],
    );
    let to = shape(
        base,
        vec![
            hunk(1, &[], 1, &["import math"]),
            hunk(
                7,
                &[],
                8,
                &[
                    "def share():",
                    "    t = total()",
                    "    if t == 0:",
                    "        return 0",
                    "    return p / t",
                ],
            ),
        ],
    );
    let map = map(&from, &to, None);
    assert_eq!(
        carried(&map, Side::New, &[7, 8, 9]),
        [Some(8), Some(9), Some(12)]
    );
    // A line of the old side the diff does not show moves by one too.
    assert_eq!(map.carry(Side::New, 3), Some(4));
    assert_eq!(map.carry(Side::New, 40), Some(43));
    // The old side does not move.
    assert_eq!(map.carry(Side::Old, 5), Some(5));
}

#[test]
fn a_changed_line_matches_nothing_however_alike_the_others() {
    let base = blob("a");
    let block = |name: &str, test: &str, value: &str| {
        [
            format!("{name}() {{"),
            format!("  if ({test})"),
            format!("    return {value};"),
            "}".into(),
        ]
    };
    let (f, g, f0) = (
        block("f", "x", "nil"),
        block("g", "y", "nil"),
        block("f", "x", "0"),
    );
    let from = shape(
        base.clone(),
        vec![hunk(1, &[], 1, &strs(&f)), hunk(11, &[], 15, &strs(&g))],
    );
    let to = shape(
        base,
        vec![hunk(1, &[], 1, &strs(&f0)), hunk(11, &[], 15, &strs(&g))],
    );
    let map = map(&from, &to, None);
    assert_eq!(map.carry(Side::New, 3), None);
    assert_eq!(
        carried(&map, Side::New, &[1, 2, 4, 17]),
        [Some(1), Some(2), Some(4), Some(17)]
    );
}

#[test]
fn a_line_is_carried_through_the_hunks_between_two_old_sides() {
    let hunks = [
        BaseHunk {
            old: 1..1,
            new: 1..3,
        },
        BaseHunk {
            old: 5..6,
            new: 7..8,
        },
        BaseHunk {
            old: 10..10,
            new: 12..15,
        },
    ];
    let (forward, backward) = Shifter::pair(&hunks).unwrap();
    let forward: Vec<_> = [1, 4, 5, 6, 9, 10, 12].map(|n| forward.shift(n)).into();
    assert_eq!(
        forward,
        [
            Some(3),
            Some(6),
            None,
            Some(8),
            Some(11),
            Some(15),
            Some(17)
        ]
    );
    let backward: Vec<_> = [1, 2, 3, 7, 11, 12, 14, 15]
        .map(|n| backward.shift(n))
        .into();
    assert_eq!(
        backward,
        [None, None, Some(1), None, Some(9), None, None, Some(10)]
    );

    let from = shape(
        blob("aaaaaaa"),
        vec![
            hunk(1, &[], 1, &["import", ""]),
            hunk(7, &[], 9, &["share"]),
        ],
    );
    let to = shape(blob("bbbbbbb"), vec![hunk(9, &[], 9, &["share"])]);
    assert_eq!(
        base_pair(&from, &to),
        Some((&b"aaaaaaa"[..], &b"bbbbbbb"[..]))
    );
    // Not knowing how the old sides differ, nothing is matched.
    assert_eq!(map(&from, &to, None).carry(Side::New, 9), None);
    let moved = [BaseHunk {
        old: 1..1,
        new: 1..3,
    }];
    let map = map(&from, &to, Some(&moved));
    assert_eq!(map.carry(Side::New, 9), Some(9));
    // The import is in the old side now: no change, and no line put in.
    assert_eq!(map.carry(Side::New, 1), None);
    assert_eq!(map.carry(Side::Old, 3), Some(5));
}

#[test]
fn hunks_between_old_sides_out_of_order_carry_nothing() {
    let from = shape(blob("1"), vec![hunk(2, &[], 2, &["x"])]);
    let to = shape(blob("2"), vec![hunk(2, &[], 2, &["x"])]);
    let bad = [
        BaseHunk {
            old: 5..6,
            new: 5..5,
        },
        BaseHunk {
            old: 2..3,
            new: 1..2,
        },
    ];
    assert_eq!(map(&from, &to, Some(&bad)).carry(Side::New, 2), None);
    let misnumbered = [BaseHunk {
        old: 5..6,
        new: 7..7,
    }];
    assert_eq!(
        map(&from, &to, Some(&misnumbered)).carry(Side::New, 2),
        None
    );
}

#[test]
fn an_old_side_that_is_the_same_or_appears() {
    let from = shape(blob("aaaaaaa"), vec![hunk(2, &["b"], 2, &["B", "C"])]);
    let to = shape(
        blob("aaaaaaaa"),
        vec![hunk(1, &[], 1, &["top"]), hunk(2, &["b"], 3, &["B", "C"])],
    );
    // No hunks between two old sides: one old side.
    let same = map(&from, &to, Some(&[]));
    assert_eq!(same.carry(Side::New, 2), Some(3));
    assert_eq!(same.carry(Side::Old, 2), Some(2));
    // An old side that is no file against a blob: nothing is carried, though lines read alike.
    let to = shape(Base::Empty, vec![hunk(1, &[], 1, &["a", "B", "C", "c"])]);
    let none = map(&from, &to, None);
    assert_eq!(carried(&none, Side::New, &[1, 2, 3, 4]), [None; 4]);
    assert_eq!(none.carry(Side::Old, 2), None);
    // Two readings of a file put in are read against one old side.
    let new = shape(Base::Empty, vec![hunk(1, &[], 1, &["a", "b"])]);
    let newer = shape(Base::Empty, vec![hunk(1, &[], 1, &["top", "a", "b"])]);
    assert_eq!(map(&new, &newer, None).carry(Side::New, 2), Some(3));
}

#[test]
fn a_new_file_once_committed_carries_nothing() {
    let file = ["fn a() {", "}", "fn b() {", "}"];
    let from = shape(Base::Empty, vec![hunk(1, &[], 1, &file)]);
    // Committed, then a function added: its brace reads like line 4's.
    let to = shape(blob("1"), vec![hunk(5, &[], 5, &["fn c() {", "}"])]);
    let map = map(&from, &to, None);
    assert_eq!(carried(&map, Side::New, &[1, 2, 3, 4]), [None; 4]);
}

#[test]
fn readings_against_unknown_old_sides_match_by_text() {
    let from = shape(
        Base::Unknown,
        vec![hunk(10, &[], 10, &["a"]), hunk(20, &[], 21, &["b"])],
    );
    let to = shape(
        Base::Unknown,
        vec![
            hunk(2, &[], 2, &["x", "y"]),
            hunk(10, &[], 12, &["a"]),
            hunk(20, &[], 23, &["b"]),
        ],
    );
    let map = map(&from, &to, None);
    assert_eq!(base_pair(&from, &to), None);
    assert_eq!(map.carry(Side::New, 10), Some(12));
    assert_eq!(map.carry(Side::New, 21), Some(23));
    // Nothing was taken out to be matched.
    assert_eq!(map.carry(Side::Old, 10), None);
    // A line rewritten is still nowhere.
    let rewritten = shape(
        Base::Unknown,
        vec![hunk(10, &[], 10, &["A"]), hunk(20, &[], 21, &["b"])],
    );
    assert_eq!(
        LineMap::new(&from, &rewritten, None, lcs).carry(Side::New, 10),
        None
    );
}

#[test]
fn a_new_file_is_matched_whole() {
    let lines: Vec<String> = (1..=400).map(|n| format!("line {n}")).collect();
    let mut first = vec!["}".to_owned(), "}".to_owned()];
    first.extend(lines.iter().cloned());
    let mut next = vec!["new".to_owned(), "}".to_owned(), "}".to_owned()];
    next.extend(lines[..199].iter().cloned());
    next.push("changed".into());
    next.extend(lines[200..].iter().cloned());
    let from = shape(Base::Empty, vec![hunk(1, &[], 1, &strs(&first))]);
    let to = shape(Base::Empty, vec![hunk(1, &[], 1, &strs(&next))]);
    let map = map(&from, &to, None);
    assert_eq!(map.carry(Side::New, 1), Some(2));
    assert_eq!(map.carry(Side::New, 3), Some(4));
    assert_eq!(map.carry(Side::New, 202), None);
    assert_eq!(map.carry(Side::New, 402), Some(403));
}

#[test]
fn a_matcher_that_lies_matches_nothing() {
    let base = blob("1");
    let from = shape(base.clone(), vec![hunk(1, &[], 1, &["a", "b"])]);
    let to = shape(base, vec![hunk(1, &[], 1, &["a", "b"])]);
    for lie in [vec![(0, 1)], vec![(1, 1), (0, 0)], vec![(0, 0), (5, 1)]] {
        let map = LineMap::new(&from, &to, None, |_: &[&[u8]], _: &[&[u8]]| lie.clone());
        assert_eq!(carried(&map, Side::New, &[1, 2, 3]), [None, None, Some(3)]);
    }
}

#[test]
fn numbers_near_the_last_one_neither_overflow_nor_pass() {
    let top = u32::MAX - 1;
    assert_eq!(
        Shape::new(Base::Unknown, vec![hunk(top, &["a"], top, &[])]),
        Err(ShapeError::TooLong)
    );
    let from = shape(blob("1"), vec![hunk(1, &[], 1, &["a", "b"])]);
    let to = shape(
        blob("1"),
        vec![hunk(2, &["x"], 2, &[]), hunk(top - 2, &[], top - 3, &["y"])],
    );
    let map = map(&from, &to, None);
    for number in [1, 2, 3, 5, top - 3, top - 1, top, u32::MAX] {
        map.carry(Side::New, number);
        map.carry(Side::Old, number);
    }
    let edge = shape(Base::Unknown, vec![hunk(top - 1, &["x"], top - 1, &[])]);
    let by_text = LineMap::new(&edge, &edge, None, lcs);
    assert_eq!(by_text.carry(Side::Old, top - 1), Some(top - 1));
}

mod properties;
