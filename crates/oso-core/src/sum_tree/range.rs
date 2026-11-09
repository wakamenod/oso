//! The summary of a run of items, read from the cached summaries without copying a node,
//! for a caller that needs what lies between two places but not a tree of it (ADR 0101).
//!
//! Must not depend on: any module of this crate, or what its items hold.

use std::ops::Range;

use super::{Dimension, Item, Node, SumTree, Summary};

impl<T: Item> SumTree<T> {
    /// The summary of the items that splitting at `range.start` and at `range.end`, both
    /// with [`Bias::Right`](super::Bias::Right), would cut out: those that end after
    /// `range.start` and no later than `range.end`. O(log n), and it allocates nothing.
    pub(crate) fn summary_in<D>(&self, range: &Range<D>) -> T::Summary
    where
        D: Dimension<T::Summary> + Ord,
    {
        let mut total = T::Summary::default();
        let mut position = D::default();
        self.0.add_in(range, &mut position, &mut total);
        total
    }
}

impl<T: Item> Node<T> {
    /// Adds to `total` the summaries of this node's items in `range`, with `position` at
    /// the node's start, and moves `position` past what it went over. Returns false once
    /// past `range.end`, so that the caller stops.
    fn add_in<D>(&self, range: &Range<D>, position: &mut D, total: &mut T::Summary) -> bool
    where
        D: Dimension<T::Summary> + Ord,
    {
        let (summaries, children) = match self {
            Node::Leaf { item_summaries, .. } => (item_summaries, None),
            Node::Internal {
                child_summaries,
                children,
                ..
            } => (child_summaries, Some(children)),
        };
        for (index, summary) in summaries.iter().enumerate() {
            // Past the end: an item of no size at the end still ends no later than it.
            if *position > range.end {
                return false;
            }
            let mut end = position.clone();
            end.add_summary(summary);
            if end <= range.start {
                *position = end;
                continue;
            }
            if *position > range.start && end <= range.end {
                total.add_summary(summary);
                *position = end;
                continue;
            }
            // It straddles an end of the range: an item ends past it, or a child holds
            // items on both sides.
            match children.and_then(|children| children.get(index)) {
                Some(child) => {
                    if !child.0.add_in(range, position, total) {
                        return false;
                    }
                    *position = end;
                }
                None => {
                    if end <= range.end {
                        total.add_summary(summary);
                    }
                    *position = end;
                }
            }
        }
        true
    }
}
