//! Decisions on hunks (ADR 0122, "Decisions", Q19): accepted, rejected and viewed, keyed by
//! the exact text of the hunk's removed and added lines, not their numbers.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.
//!
//! Each hunk of the current reading has its mark, so a lookup costs nothing. When a new reading
//! comes, the marks are carried to it by content: a hash finds the candidates and the text
//! decides, so a mark never moves to changed code. When the agent rewrites a hunk, no hunk has
//! its text any more and the mark drops. Hunks with the same text keep their own marks, the
//! first of them in one reading going to the first in the next, and so on.

use std::collections::HashMap;
use std::hash::BuildHasher;

use super::shape::Hunk;

/// What the person decided about a hunk. Viewed is kept apart: a hunk can be accepted and
/// viewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Decision {
    Accepted,
    Rejected,
}

/// What the person marked on one hunk, and whether it is new since the reading before and not
/// yet looked at, which is no mark of theirs and drops with no word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Mark {
    pub(super) decision: Option<Decision>,
    pub(super) viewed: bool,
    pub(super) changed: bool,
}

impl Mark {
    pub(super) fn is_set(self) -> bool {
        self.decision.is_some() || self.viewed
    }
}

/// The hash a hunk's content is found by.
pub(super) fn hash(hasher: &impl BuildHasher, hunk: &Hunk) -> u64 {
    hasher.hash_one((&hunk.removed[..], &hunk.added[..]))
}

fn same(a: &Hunk, b: &Hunk) -> bool {
    a.removed == b.removed && a.added == b.added
}

/// The marks of one reading's hunks.
pub(super) struct Marked<'a> {
    pub(super) hunks: &'a [Hunk],
    pub(super) hashes: &'a [u64],
    pub(super) marks: &'a [Mark],
}

/// Hunks of two readings with one text, in order.
struct Group {
    from: Vec<usize>,
    to: Vec<usize>,
}

/// The marks of `from` carried to the hunks `to`, whose hashes are `hashes`, and how many
/// marked hunks found no hunk with their text. Hunks of one text pair in order, the k-th with
/// the k-th; a hunk of `to` that pairs with none is new since `from`, and marked changed (ADR
/// 0122, step 4).
pub(super) fn carry_marks(from: &Marked<'_>, to: &[Hunk], hashes: &[u64]) -> (Vec<Mark>, usize) {
    let mut marks = vec![Mark::default(); to.len()];
    let mut groups: HashMap<u64, Vec<Group>> = HashMap::new();
    let sides = [(from.hunks, from.hashes, true), (to, hashes, false)];
    for (hunks, hashes, is_from) in sides {
        for (index, (hunk, hash)) in hunks.iter().zip(hashes).enumerate() {
            let bucket = groups.entry(*hash).or_default();
            let found = bucket.iter_mut().find(|group| {
                let first = group.from.first().map(|at| &from.hunks[*at]);
                first.is_some_and(|first| same(first, hunk))
            });
            match (found, is_from) {
                (Some(group), true) => group.from.push(index),
                (Some(group), false) => group.to.push(index),
                (None, true) => bucket.push(Group {
                    from: vec![index],
                    to: Vec::new(),
                }),
                (None, false) => marks[index].changed = true,
            }
        }
    }
    let mut dropped = 0;
    for group in groups.values().flatten() {
        for (k, &index) in group.from.iter().enumerate() {
            let mark = from.marks[index];
            match group.to.get(k) {
                Some(&there) => marks[there] = mark,
                None => dropped += usize::from(mark.is_set()),
            }
        }
        // More of one text than before: those past the ones before are new.
        for &there in group.to.iter().skip(group.from.len()) {
            marks[there].changed = true;
        }
    }
    (marks, dropped)
}
