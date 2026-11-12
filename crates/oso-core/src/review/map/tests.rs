//! The tests of the line map: cases ported from the author's previous review, and properties
//! checked against a reference that walks the new side line by line, as that review did,
//! where the map walks the hunks' edges.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use std::collections::HashMap;

use proptest::prelude::*;

use super::super::shape::ShapeError;
use super::super::tests::{base_hunks, blob, diff, hunk, lcs};
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
    // An old side that is no file: only lines put in are matched.
    let to = shape(Base::Empty, vec![hunk(1, &[], 1, &["a", "B", "C", "c"])]);
    let none = map(&from, &to, None);
    assert_eq!(none.carry(Side::New, 2), Some(2));
    assert_eq!(none.carry(Side::New, 3), Some(3));
    assert_eq!(none.carry(Side::Old, 2), None);
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

/// A line of the new side, as the reference sees it.
#[derive(Clone, Copy)]
enum Token<'a> {
    Old(u32),
    Put(&'a [u8]),
}

/// The new side of a reading, line by line down to its last change, as the previous review
/// built it.
struct Tokens<'a> {
    tokens: Vec<Token<'a>>,
    next: u32,
    positions: HashMap<u32, usize>,
}

impl<'a> Tokens<'a> {
    fn of(shape: &'a Shape) -> Tokens<'a> {
        let mut tokens = Vec::new();
        let mut positions = HashMap::new();
        let mut old = 1;
        for hunk in shape.hunks() {
            while old < hunk.old_start {
                positions.insert(old, tokens.len());
                tokens.push(Token::Old(old));
                old += 1;
            }
            old += hunk.removed.len() as u32;
            tokens.extend(hunk.added.iter().map(|line| Token::Put(line)));
        }
        Tokens {
            tokens,
            next: old,
            positions,
        }
    }

    fn token(&self, index: usize) -> Token<'a> {
        self.tokens
            .get(index)
            .copied()
            .unwrap_or(Token::Old(self.next + (index - self.tokens.len()) as u32))
    }

    fn index(&self, number: u32) -> Option<usize> {
        self.positions.get(&number).copied().or_else(|| {
            (number >= self.next).then(|| self.tokens.len() + (number - self.next) as usize)
        })
    }
}

/// Where each line of `from` goes in `to`, line by line: the reference the map is checked
/// against. `forward` and `backward` carry old lines between the old sides.
fn reference(
    from: &Shape,
    to: &Shape,
    forward: &dyn Fn(u32) -> Option<u32>,
    backward: &dyn Fn(u32) -> Option<u32>,
    side: Side,
    number: u32,
) -> Option<u32> {
    if side == Side::Old {
        return forward(number);
    }
    let (a, b) = (Tokens::of(from), Tokens::of(to));
    let mut pairs = Vec::new();
    for (i, token) in a.tokens.iter().enumerate() {
        if let Token::Old(old) = *token
            && let Some(j) = forward(old).and_then(|there| b.index(there))
        {
            pairs.push((i, j));
        }
    }
    for (j, token) in b.tokens.iter().enumerate() {
        if let Token::Old(old) = *token
            && let Some(i) = backward(old).and_then(|here| a.index(here))
            && i >= a.tokens.len()
        {
            pairs.push((i, j));
        }
    }
    let mut map: Vec<Option<usize>> = vec![None; a.tokens.len()];
    let (mut i0, mut j0) = (0, 0);
    pairs.push((a.tokens.len(), b.tokens.len()));
    for (pi, pj) in pairs {
        let (i1, j1) = (pi.min(a.tokens.len()), pj.min(b.tokens.len()));
        if i0 < i1 && j0 < j1 {
            let (ai, at) = put_in(&a, i0..i1);
            let (bj, bt) = put_in(&b, j0..j1);
            for (i, j) in lcs(&at, &bt) {
                map[ai[i]] = Some(bj[j]);
            }
        }
        if pi < a.tokens.len() {
            map[pi] = Some(pj);
        }
        (i0, j0) = (i0.max(pi + 1), j0.max(pj + 1));
    }
    let i = (number - 1) as usize;
    if i < map.len() {
        return map[i].map(|j| j as u32 + 1);
    }
    let Token::Old(old) = a.token(i) else {
        return None;
    };
    Some(b.index(forward(old)?)? as u32 + 1)
}

/// The lines put in among `range` of `tokens`, by index.
fn put_in<'a>(tokens: &Tokens<'a>, range: std::ops::Range<usize>) -> (Vec<usize>, Vec<&'a [u8]>) {
    range
        .filter_map(|k| match tokens.tokens[k] {
            Token::Put(text) => Some((k, text)),
            Token::Old(_) => None,
        })
        .unzip()
}

/// Lines from a small alphabet, so that most of them read like many others.
fn text() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(
        prop::sample::select(vec!["}", "", "a", "b", "c", "d"]).prop_map(str::to_owned),
        0..24,
    )
}

/// An edit of a text: for each line, whether it is kept and what is put in front of it.
fn edit() -> impl Strategy<Value = (Vec<(bool, Vec<String>)>, Vec<String>)> {
    let line = prop::sample::select(vec!["}", "", "a", "b", "e", "f"]).prop_map(str::to_owned);
    let step = (
        prop::bool::weighted(0.75),
        proptest::collection::vec(line.clone(), 0..3),
    );
    (
        proptest::collection::vec(step, 24),
        proptest::collection::vec(line, 0..3),
    )
}

fn apply_edit(
    text: &[String],
    (steps, tail): &(Vec<(bool, Vec<String>)>, Vec<String>),
) -> Vec<String> {
    let mut out = Vec::new();
    for (line, (keep, before)) in text.iter().zip(steps) {
        out.extend(before.iter().cloned());
        if *keep {
            out.push(line.clone());
        }
    }
    out.extend(tail.iter().cloned());
    out
}

fn strs(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).collect()
}

/// Checks `map` against the texts: a line carried lands on a line with its text, in order.
fn check_sound(
    map: &LineMap,
    side: Side,
    from: &[String],
    to: &[String],
) -> Result<(), TestCaseError> {
    let mut last = 0;
    for number in 1..=from.len() as u32 {
        if let Some(there) = map.carry(side, number) {
            prop_assert!(
                there > last,
                "line {} went to {} after {}",
                number,
                there,
                last
            );
            prop_assert!((there as usize) <= to.len());
            prop_assert_eq!(&from[number as usize - 1], &to[there as usize - 1]);
            last = there;
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn a_reading_maps_to_itself(base in text(), first in edit()) {
        let new = apply_edit(&base, &first);
        let shape = diff(&strs(&base), &strs(&new), blob("1"));
        let map = map(&shape, &shape, None);
        for number in 1..=new.len() as u32 + 3 {
            prop_assert_eq!(map.carry(Side::New, number), Some(number));
        }
        for number in 1..=base.len() as u32 + 3 {
            prop_assert_eq!(map.carry(Side::Old, number), Some(number));
        }
    }

    #[test]
    fn readings_of_one_old_side_match_the_reference(
        base in text(), first in edit(), second in edit(),
    ) {
        let (new1, new2) = (apply_edit(&base, &first), apply_edit(&base, &second));
        let from = diff(&strs(&base), &strs(&new1), blob("1"));
        let to = diff(&strs(&base), &strs(&new2), blob("1"));
        let map = map(&from, &to, None);
        let same = |number| Some(number);
        for number in 1..=new1.len() as u32 + 3 {
            prop_assert_eq!(
                map.carry(Side::New, number),
                reference(&from, &to, &same, &same, Side::New, number),
                "line {}", number
            );
        }
        check_sound(&map, Side::New, &new1, &new2)?;
    }

    #[test]
    fn readings_of_a_moved_old_side_match_the_reference(
        base in text(), moved in edit(), first in edit(), second in edit(),
    ) {
        let base2 = apply_edit(&base, &moved);
        let (new1, new2) = (apply_edit(&base, &first), apply_edit(&base2, &second));
        let from = diff(&strs(&base), &strs(&new1), blob("1"));
        let to = diff(&strs(&base2), &strs(&new2), blob("2"));
        let hunks = base_hunks(&strs(&base), &strs(&base2));
        let map = map(&from, &to, Some(&hunks));
        let (forward, backward) = Shifter::pair(&hunks).unwrap();
        let forward = |number| forward.shift(number);
        let backward = |number| backward.shift(number);
        for number in 1..=new1.len() as u32 + 3 {
            prop_assert_eq!(
                map.carry(Side::New, number),
                reference(&from, &to, &forward, &backward, Side::New, number),
                "line {}", number
            );
        }
        check_sound(&map, Side::New, &new1, &new2)?;
        check_sound(&map, Side::Old, &base, &base2)?;
    }

    #[test]
    fn a_new_file_matches_the_reference(first in text(), base2 in text(), second in edit()) {
        let new2 = apply_edit(&base2, &second);
        let from = diff(&[], &strs(&first), Base::Empty);
        let to = diff(&strs(&base2), &strs(&new2), blob("2"));
        let map = map(&from, &to, None);
        let nothing = |_| None;
        for number in 1..=first.len() as u32 + 3 {
            prop_assert_eq!(
                map.carry(Side::New, number),
                reference(&from, &to, &nothing, &nothing, Side::New, number)
            );
        }
        check_sound(&map, Side::New, &first, &new2)?;
    }

    #[test]
    fn readings_by_text_carry_only_to_equal_lines(
        base in text(), first in edit(), second in edit(),
    ) {
        let (new1, new2) = (apply_edit(&base, &first), apply_edit(&base, &second));
        let from = diff(&strs(&base), &strs(&new1), Base::Unknown);
        let to = diff(&strs(&base), &strs(&new2), Base::Unknown);
        let map = map(&from, &to, None);
        let mut last = 0;
        for hunk in from.hunks() {
            for number in hunk.new_start..hunk.new_start + hunk.added.len() as u32 {
                if let Some(there) = map.carry(Side::New, number) {
                    prop_assert!(there > last);
                    prop_assert_eq!(from.line(Side::New, number), to.line(Side::New, there));
                    last = there;
                }
            }
        }
    }

    #[test]
    fn a_lying_matcher_never_carries_to_another_text(
        base in text(), first in edit(), second in edit(),
        lies in proptest::collection::vec((0usize..30, 0usize..30), 0..8),
    ) {
        let (new1, new2) = (apply_edit(&base, &first), apply_edit(&base, &second));
        for base_of in [blob("1"), Base::Unknown] {
            let from = diff(&strs(&base), &strs(&new1), base_of.clone());
            let to = diff(&strs(&base), &strs(&new2), base_of);
            let map = LineMap::new(&from, &to, None, |_: &[&[u8]], _: &[&[u8]]| lies.clone());
            for number in 1..=new1.len() as u32 {
                if let Some(there) = map.carry(Side::New, number) {
                    prop_assert!((there as usize) <= new2.len());
                    prop_assert_eq!(&new1[number as usize - 1], &new2[there as usize - 1]);
                }
            }
        }
    }
}
