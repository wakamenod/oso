//! Replacing a run of items, in place when one leaf holds it, so that an edit copies only
//! the path to that leaf and a command's edits at many cursors share what they copy
//! (ADR 0091).
//!
//! Must not depend on: any module of this crate, or what its items hold.

use std::ops::Range;
use std::sync::Arc;

use super::{
    Bias, Dimension, Item, MAX_CHILDREN, MIN_CHILDREN, Node, SumTree, seek_index, split_overflow,
    sum,
};

/// Where a replacement goes when one leaf holds it: the child taken at each level, and the
/// items of that leaf it replaces.
struct InLeaf {
    path: Vec<usize>,
    items: Range<usize>,
}

impl<T: Item> SumTree<T> {
    /// Replaces the items that splitting at `range.start` and at `range.end`, both with
    /// [`Bias::Right`], would cut out with `items`, as two splits and two appends would.
    ///
    /// When one leaf holds them and keeps a size it may have, only the path to that leaf
    /// is copied, and on a tree whose path an earlier edit copied already, nothing is, so
    /// the edits of one command share their copies. Otherwise it splits and appends.
    pub(crate) fn replace_items<D>(&mut self, range: Range<D>, items: Vec<T>)
    where
        D: Dimension<T::Summary> + Ord,
    {
        let Some(place) = self.in_leaf(&range, items.len()) else {
            let (mut tree, _) = self.split(&range.start, Bias::Right);
            let (_, right) = self.split(&range.end, Bias::Right);
            tree.append(items.into_iter().collect());
            tree.append(right);
            *self = tree;
            return;
        };
        if let Some(right) = replace_at(self, &place.path, place.items, items) {
            let left = std::mem::take(self);
            *self = Self::internal(vec![left, right]);
        }
    }

    /// Where `range` falls if one leaf holds every item it cuts out, and that leaf keeps
    /// between `MIN_CHILDREN` (none for the root) and twice `MAX_CHILDREN` entries once
    /// `count` items take their place. O(log n), and it copies nothing.
    fn in_leaf<D>(&self, range: &Range<D>, count: usize) -> Option<InLeaf>
    where
        D: Dimension<T::Summary> + Ord,
    {
        let mut path = Vec::with_capacity(usize::from(self.height()));
        let mut node = &*self.0;
        let mut position = D::default();
        loop {
            match node {
                Node::Internal {
                    children,
                    child_summaries,
                    ..
                } => {
                    let mut start = position.clone();
                    let index = seek_index(child_summaries, &range.start, Bias::Right, &mut start)
                        .min(children.len() - 1);
                    for summary in &child_summaries[..index] {
                        position.add_summary(summary);
                    }
                    path.push(index);
                    node = &children[index].0;
                }
                Node::Leaf {
                    item_summaries,
                    summary,
                    ..
                } => {
                    let mut end = position.clone();
                    end.add_summary(summary);
                    // At the leaf's end, a split would also cut out the items after it
                    // that take nothing in `D`.
                    let next_is_past = || {
                        self.first_after(&path).is_none_or(|next| {
                            let mut next_end = end.clone();
                            next_end.add_summary(next);
                            next_end > end
                        })
                    };
                    if range.end > end || (range.end == end && !next_is_past()) {
                        return None;
                    }
                    let first = seek_index(
                        item_summaries,
                        &range.start,
                        Bias::Right,
                        &mut position.clone(),
                    );
                    let last = seek_index(item_summaries, &range.end, Bias::Right, &mut position);
                    let len = item_summaries.len() - (last - first) + count;
                    let fewest = if path.is_empty() { 0 } else { MIN_CHILDREN };
                    if len < fewest || len > 2 * MAX_CHILDREN {
                        return None;
                    }
                    return Some(InLeaf {
                        path,
                        items: first..last,
                    });
                }
            }
        }
    }
}

impl<T: Item> SumTree<T> {
    /// The summary of the first item after the leaf `path` leads to, if there is one.
    fn first_after(&self, path: &[usize]) -> Option<&T::Summary> {
        // The child after the one taken, at the deepest node on the path that has one.
        let mut after = None;
        let mut node = &*self.0;
        for &index in path {
            let Node::Internal { children, .. } = node else {
                return None;
            };
            after = children.get(index + 1).or(after);
            node = &children[index].0;
        }
        let mut node = &*after?.0;
        loop {
            match node {
                Node::Internal { children, .. } => node = &children.first()?.0,
                Node::Leaf { item_summaries, .. } => return item_summaries.first(),
            }
        }
    }
}

/// Puts `items` in place of `range` in the leaf `path` leads to, copying each node on the
/// way that another tree shares, and updates the summaries above it. Returns a new right
/// sibling of `tree` if it overflowed.
fn replace_at<T: Item>(
    tree: &mut SumTree<T>,
    path: &[usize],
    range: Range<usize>,
    items: Vec<T>,
) -> Option<SumTree<T>> {
    let node = Arc::make_mut(&mut tree.0);
    match node {
        Node::Leaf {
            summary,
            items: leaf,
            item_summaries,
        } => {
            let summaries: Vec<_> = items.iter().map(Item::summary).collect();
            leaf.splice(range.clone(), items);
            item_summaries.splice(range, summaries);
            *summary = sum(item_summaries);
        }
        Node::Internal {
            summary,
            children,
            child_summaries,
            ..
        } => {
            let Some((&index, rest)) = path.split_first() else {
                debug_assert!(false, "a path ends at a leaf");
                return None;
            };
            let overflow = replace_at(&mut children[index], rest, range, items);
            child_summaries[index] = children[index].summary().clone();
            if let Some(sibling) = overflow {
                child_summaries.insert(index + 1, sibling.summary().clone());
                children.insert(index + 1, sibling);
            }
            *summary = sum(child_summaries);
        }
    }
    split_overflow(node, false)
}
