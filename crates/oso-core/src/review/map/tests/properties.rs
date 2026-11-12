//! Properties of the line map, checked against a reference that walks the new side line by
//! line, as the author's previous review did, where the map walks the hunks' edges.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use std::collections::HashMap;

use proptest::prelude::*;

use super::*;

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
    fn a_new_file_against_a_blob_carries_nothing(
        first in text(), base2 in text(), second in edit(),
    ) {
        let new2 = apply_edit(&base2, &second);
        let from = diff(&[], &strs(&first), Base::Empty);
        let to = diff(&strs(&base2), &strs(&new2), blob("2"));
        for (from, to) in [(&from, &to), (&to, &from)] {
            let map = map(from, to, None);
            for number in 1..=first.len().max(new2.len()) as u32 + 3 {
                prop_assert_eq!(map.carry(Side::New, number), None);
                prop_assert_eq!(map.carry(Side::Old, number), None);
            }
        }
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
