//! Tests of composing batches, against texts whose every element is told apart.
//!
//! Must not depend on: `highlights`.

use proptest::prelude::*;

use super::compose;
use crate::syntax::Splice;

fn splice(start: usize, old_len: usize, new_len: usize) -> Splice {
    Splice {
        start,
        old_len,
        new_len,
    }
}

/// Applies `batch` to `text`, highest first, with new elements numbered from `next`.
fn apply(text: &[u32], batch: &[Splice], next: &mut u32) -> Vec<u32> {
    let mut out = text.to_vec();
    for splice in batch.iter().rev() {
        let new: Vec<u32> = (0..splice.new_len)
            .map(|_| {
                *next += 1;
                *next
            })
            .collect();
        out.splice(splice.start..splice.start + splice.old_len, new);
    }
    out
}

/// A batch for a text of `len` elements, drawn as gaps and lengths.
fn batch(len: usize) -> impl Strategy<Value = Vec<Splice>> {
    prop::collection::vec((0usize..6, 0usize..5, 0usize..5), 0..5).prop_map(move |drawn| {
        let mut out = Vec::new();
        let mut at = 0;
        for (gap, old_len, new_len) in drawn {
            let start = at + gap;
            if start > len {
                break;
            }
            let old_len = old_len.min(len - start);
            out.push(splice(start, old_len, new_len));
            at = start + old_len;
        }
        out
    })
}

#[test]
fn a_typed_run_of_characters_is_one_splice() {
    let typed = compose(&[splice(4, 0, 1)], &[splice(5, 0, 1)]).unwrap();
    assert_eq!(typed, [splice(4, 0, 2)]);
    let deleted = compose(&[splice(4, 1, 0)], &[splice(3, 1, 0)]).unwrap();
    assert_eq!(deleted, [splice(3, 2, 0)]);
}

#[test]
fn edits_apart_stay_apart() {
    let both = compose(&[splice(2, 1, 3)], &[splice(10, 2, 0)]).unwrap();
    assert_eq!(both, [splice(2, 1, 3), splice(8, 2, 0)]);
    let before = compose(&[splice(10, 0, 2)], &[splice(1, 1, 1)]).unwrap();
    assert_eq!(before, [splice(1, 1, 1), splice(10, 0, 2)]);
}

#[test]
fn a_batch_out_of_order_gives_none() {
    assert!(compose(&[splice(5, 2, 0), splice(6, 0, 1)], &[]).is_none());
    assert!(compose(&[], &[splice(5, 2, 0), splice(4, 0, 1)]).is_none());
    assert_eq!(compose(&[], &[]), Some(Vec::new()));
}

proptest! {
    /// The composed batch replaces exactly the elements that did not survive both, and keeps
    /// the rest where both batches put them.
    #[test]
    fn a_composed_batch_does_what_both_do(
        len in 0usize..30,
        first in batch(30),
        then_drawn in batch(60),
    ) {
        let text: Vec<u32> = (0..len as u32).collect();
        let first: Vec<Splice> = first.into_iter().filter(|s| s.start + s.old_len <= len).collect();
        let mut next = 1000;
        let middle = apply(&text, &first, &mut next);
        let then: Vec<Splice> = then_drawn
            .into_iter()
            .filter(|s| s.start + s.old_len <= middle.len())
            .collect();
        let end = apply(&middle, &then, &mut next);
        let composed = compose(&first, &then).unwrap();
        // Ascending and apart, in the first text.
        let mut at = 0;
        for splice in &composed {
            prop_assert!(splice.start >= at);
            at = splice.start + splice.old_len;
        }
        prop_assert!(at <= len);
        // Outside the composed splices, the elements are those that survived, in order.
        let mut kept_before = Vec::new();
        let mut kept_after = Vec::new();
        let (mut old_at, mut new_at) = (0, 0);
        for splice in &composed {
            let gap = splice.start - old_at;
            kept_before.extend_from_slice(&text[old_at..splice.start]);
            kept_after.extend_from_slice(&end[new_at..new_at + gap]);
            old_at = splice.start + splice.old_len;
            new_at += gap + splice.new_len;
        }
        kept_before.extend_from_slice(&text[old_at..]);
        kept_after.extend_from_slice(&end[new_at.min(end.len())..]);
        prop_assert_eq!(end.len(), new_at + (len - old_at));
        prop_assert_eq!(&kept_before, &kept_after);
        let survived: Vec<u32> = end.iter().copied().filter(|&e| (e as usize) < len).collect();
        prop_assert_eq!(kept_before, survived);
    }
}
