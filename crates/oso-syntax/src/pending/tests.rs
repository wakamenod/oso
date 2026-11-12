//! Tests of the pending ranges: joining, cutting, and mapping through a batch (ADR 0111).
//!
//! Must not depend on: the worker; these call the module directly.

use oso_core::syntax::Splice;
use proptest::prelude::*;

use super::Pending;

fn splice(start: usize, old_len: usize, new_len: usize) -> Splice {
    Splice {
        start,
        old_len,
        new_len,
    }
}

fn pending(ranges: &[std::ops::Range<usize>]) -> Pending {
    let mut pending = Pending::default();
    pending.add(ranges.iter().cloned());
    pending
}

#[test]
fn a_set_kept_apart_joins_only_ranges_that_overlap() {
    let mut set = Pending::apart();
    set.add([0..2, 2..4, 3..5, 9..10]);
    assert_eq!(set.ranges(), [0..2, 2..5, 9..10]);
    // A deletion between two makes them touch, and they stay apart.
    set.map(&[splice(5, 4, 0)]);
    assert_eq!(set.ranges(), [0..2, 2..5, 5..6]);
}

#[test]
fn swapping_replaces_the_ranges_that_meet_and_says_how_far_they_reached() {
    let mut set = Pending::apart();
    set.add([0..2, 3..6, 8..12, 20..22]);
    assert_eq!(set.swap(4..9, std::slice::from_ref(&(5..7))), 3..12);
    assert_eq!(set.ranges(), [0..2, 5..7, 20..22]);
    // Ranges that only touch it are not replaced.
    assert_eq!(set.swap(7..20, &[]), 7..20);
    assert_eq!(set.ranges(), [0..2, 5..7, 20..22]);
}

#[test]
fn adding_joins_ranges_that_meet() {
    let mut set = pending(&[0..2, 5..7]);
    set.add([1..3, 3..4, 7..9, 20..20]);
    assert_eq!(set.ranges(), [0..4, 5..9]);
}

#[test]
fn removing_cuts_ranges() {
    let mut set = pending(&[0..10, 20..30]);
    set.remove(5..25);
    assert_eq!(set.ranges(), [0..5, 25..30]);
    set.remove(0..100);
    assert!(set.is_empty());
}

#[test]
fn mapping_shifts_and_cuts() {
    // An insertion before, a replacement across the start, a deletion inside.
    let mut set = pending(&[10..20, 30..40]);
    set.map(&[splice(0, 0, 3), splice(8, 4, 1), splice(32, 2, 0)]);
    assert_eq!(set.ranges(), [12..20, 30..38]);
}

#[test]
fn text_inserted_at_either_edge_is_not_the_ranges() {
    let mut set = pending(&[10..20, 40..50]);
    set.map(&[splice(10, 0, 5), splice(20, 0, 5)]);
    assert_eq!(set.ranges(), [15..25, 50..60]);
}

#[test]
fn a_range_a_splice_took_in_whole_goes() {
    let mut set = pending(&[10..20, 40..50]);
    set.map(&[splice(5, 20, 2)]);
    assert_eq!(set.ranges(), std::slice::from_ref(&(22..32)));
}

/// Where each byte of the old text went, or `None` if a splice replaced it.
fn moved(splices: &[Splice], len: usize) -> Vec<Option<usize>> {
    let mut map = Vec::with_capacity(len);
    let (mut at, mut delta) = (0, 0isize);
    for splice in splices {
        while at < splice.start {
            map.push(Some(at.saturating_add_signed(delta)));
            at += 1;
        }
        map.extend((0..splice.old_len).map(|_| None));
        at += splice.old_len;
        delta += splice.new_len as isize - splice.old_len as isize;
    }
    while at < len {
        map.push(Some(at.saturating_add_signed(delta)));
        at += 1;
    }
    map
}

fn batch() -> impl Strategy<Value = (usize, Vec<Splice>)> {
    (1usize..200).prop_flat_map(|len| {
        let splices = prop::collection::vec((0..=len, 0usize..20, 0usize..20), 0..8).prop_map(
            move |mut raw| {
                raw.sort();
                let mut splices = Vec::new();
                let mut end = 0;
                for (start, old_len, new_len) in raw {
                    if start < end {
                        continue;
                    }
                    let old_len = old_len.min(len - start);
                    splices.push(splice(start, old_len, new_len));
                    end = start + old_len + 1;
                }
                splices
            },
        );
        (Just(len), splices)
    })
}

proptest! {
    /// A byte the batch kept is pending after it exactly when it was before.
    #[test]
    fn mapping_keeps_the_bytes_the_batch_kept(
        (len, splices) in batch(),
        raw in prop::collection::vec((0usize..200, 1usize..30), 0..6),
    ) {
        let mut ranges: Vec<_> = raw.into_iter()
            .map(|(start, len_)| start.min(len)..(start + len_).min(len))
            .collect();
        ranges.sort_by_key(|range| range.start);
        let mut set = pending(&ranges);
        let before: Vec<bool> = (0..len)
            .map(|byte| set.ranges().iter().any(|range| range.contains(&byte)))
            .collect();
        set.map(&splices);
        let ranges = set.ranges();
        prop_assert!(ranges.windows(2).all(|pair| pair[0].end < pair[1].start));
        prop_assert!(ranges.iter().all(|range| range.start < range.end));
        for (byte, to) in moved(&splices, len).into_iter().enumerate() {
            if let Some(to) = to {
                let after = ranges.iter().any(|range| range.contains(&to));
                prop_assert_eq!(after, before[byte], "byte {} at {}", byte, to);
            }
        }
    }
}
