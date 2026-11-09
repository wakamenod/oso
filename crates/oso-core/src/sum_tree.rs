//! A summarized B-tree: the data structure under the rope and every layer of
//! the display model (ADR 0022, ADR 0035).
//!
//! Must not depend on: any module of this crate, or what its items hold.
//!
//! A [`SumTree`] holds a sequence of [`Item`]s. Every node caches the
//! [`Summary`] of its subtree, so any quantity that can be accumulated from
//! summaries (a [`Dimension`], such as bytes, lines or display rows) can be
//! sought in O(log n).
//!
//! Nodes are immutable and shared through [`Arc`]: [`Clone`] is O(1), edits
//! copy only the path they change, and a tree is `Send + Sync` whenever its
//! items and summaries are. A snapshot never sees later edits.
//!
//! The design follows Zed's summarized trees; the code is from scratch.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

mod range;
mod replace;

/// Fewest children (or items) of a node other than the root.
#[cfg(not(test))]
const MIN_CHILDREN: usize = 8;
/// Small in tests, so that small inputs build deep trees.
#[cfg(test)]
const MIN_CHILDREN: usize = 2;
/// Most children (or items) of any node.
const MAX_CHILDREN: usize = 2 * MIN_CHILDREN;

/// A value accumulated over a sequence of items.
///
/// `add_summary` must be associative, and `Default` must be its identity.
/// It need not be commutative: summaries are always added in sequence order.
pub trait Summary: Clone + Default {
    /// Appends the summary of the items that follow `self`.
    fn add_summary(&mut self, other: &Self);
}

/// An element of a [`SumTree`].
pub trait Item: Clone {
    /// The summary of one item and, added up, of a subtree.
    type Summary: Summary;

    /// Computes this item's summary. Called once when the item is inserted.
    fn summary(&self) -> Self::Summary;
}

/// A position in a tree measured in one unit, accumulated from summaries.
///
/// A dimension must not decrease as summaries are added, or seeking by it
/// gives meaningless results.
pub trait Dimension<S: Summary>: Clone + Default {
    /// Advances `self` past the items that `summary` describes.
    fn add_summary(&mut self, summary: &S);

    /// The dimension of a whole summary.
    fn from_summary(summary: &S) -> Self {
        let mut dimension = Self::default();
        dimension.add_summary(summary);
        dimension
    }
}

/// Counts items, independent of their summaries' content.
impl<S: Summary> Dimension<S> for () {
    fn add_summary(&mut self, _: &S) {}
}

/// Tracks two dimensions at once, for example to seek by lines and read the
/// byte offset there. Seeking by the first component is supported directly
/// (see [`SeekTarget`]).
impl<S: Summary, D1: Dimension<S>, D2: Dimension<S>> Dimension<S> for (D1, D2) {
    fn add_summary(&mut self, summary: &S) {
        self.0.add_summary(summary);
        self.1.add_summary(summary);
    }
}

/// A value that a tree can be sought by, compared against the position the
/// seek has reached in dimension `D`.
pub trait SeekTarget<S: Summary, D: Dimension<S>> {
    /// Compares the target with `position`.
    fn cmp_position(&self, position: &D) -> Ordering;
}

impl<S: Summary, D: Dimension<S> + Ord> SeekTarget<S, D> for D {
    fn cmp_position(&self, position: &D) -> Ordering {
        self.cmp(position)
    }
}

impl<S, D1, D2> SeekTarget<S, (D1, D2)> for D1
where
    S: Summary,
    D1: Dimension<S> + Ord,
    D2: Dimension<S>,
{
    fn cmp_position(&self, position: &(D1, D2)) -> Ordering {
        self.cmp(&position.0)
    }
}

/// Which side wins when a target falls exactly on a boundary between two
/// items, or inside something that cannot be split (a wide character, a tab).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Bias {
    /// The earlier item, or the start of the unit.
    #[default]
    Left,
    /// The later item, or the end of the unit.
    Right,
}

/// Whether a seek for `target` stops in the item that ends at `end`.
fn stops_before<S, D, T>(target: &T, end: &D, bias: Bias) -> bool
where
    S: Summary,
    D: Dimension<S>,
    T: SeekTarget<S, D> + ?Sized,
{
    match target.cmp_position(end) {
        Ordering::Less => true,
        Ordering::Equal => bias == Bias::Left,
        Ordering::Greater => false,
    }
}

/// A persistent B-tree of items with cached summaries. See the module docs.
pub struct SumTree<T: Item>(Arc<Node<T>>);

#[derive(Clone)]
enum Node<T: Item> {
    Leaf {
        summary: T::Summary,
        items: Vec<T>,
        item_summaries: Vec<T::Summary>,
    },
    Internal {
        /// Distance to the leaves; a leaf has height 0.
        height: u8,
        summary: T::Summary,
        children: Vec<SumTree<T>>,
        child_summaries: Vec<T::Summary>,
    },
}

impl<T: Item> Node<T> {
    fn summary(&self) -> &T::Summary {
        match self {
            Node::Leaf { summary, .. } | Node::Internal { summary, .. } => summary,
        }
    }

    fn height(&self) -> u8 {
        match self {
            Node::Leaf { .. } => 0,
            Node::Internal { height, .. } => *height,
        }
    }

    /// Items in a leaf, children in an internal node.
    fn len(&self) -> usize {
        match self {
            Node::Leaf { items, .. } => items.len(),
            Node::Internal { children, .. } => children.len(),
        }
    }

    fn summaries(&self) -> &[T::Summary] {
        match self {
            Node::Leaf { item_summaries, .. } => item_summaries,
            Node::Internal {
                child_summaries, ..
            } => child_summaries,
        }
    }
}

fn sum<S: Summary>(summaries: &[S]) -> S {
    let mut total = S::default();
    for summary in summaries {
        total.add_summary(summary);
    }
    total
}

impl<T: Item> Clone for SumTree<T> {
    /// O(1): the clone shares every node.
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Item> Default for SumTree<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Item + fmt::Debug> fmt::Debug for SumTree<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: Item> FromIterator<T> for SumTree<T> {
    /// Builds a balanced tree bottom-up in O(n).
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let items: Vec<T> = iter.into_iter().collect();
        if items.is_empty() {
            return Self::new();
        }
        let mut level: Vec<SumTree<T>> =
            balanced_groups(items).into_iter().map(Self::leaf).collect();
        while level.len() > 1 {
            level = balanced_groups(level)
                .into_iter()
                .map(Self::internal)
                .collect();
        }
        level.pop().unwrap_or_default()
    }
}

/// Splits `values` into the fewest groups of at most `MAX_CHILDREN`, with
/// sizes as even as possible. Every group has at least `MIN_CHILDREN` when
/// there is more than one.
fn balanced_groups<X>(values: Vec<X>) -> Vec<Vec<X>> {
    let total = values.len();
    let count = total.div_ceil(MAX_CHILDREN).max(1);
    let (base, extra) = (total / count, total % count);
    let mut groups = Vec::with_capacity(count);
    let mut values = values.into_iter();
    for index in 0..count {
        let size = base + usize::from(index < extra);
        groups.push(values.by_ref().take(size).collect());
    }
    groups
}

impl<T: Item> SumTree<T> {
    /// An empty tree.
    pub fn new() -> Self {
        Self::leaf(Vec::new())
    }

    /// A tree holding one item.
    pub fn from_item(item: T) -> Self {
        Self::leaf(vec![item])
    }

    fn leaf(items: Vec<T>) -> Self {
        let item_summaries = items.iter().map(Item::summary).collect();
        Self::leaf_from_parts(items, item_summaries)
    }

    /// A leaf from items and their already computed summaries.
    fn leaf_from_parts(items: Vec<T>, item_summaries: Vec<T::Summary>) -> Self {
        Self(Arc::new(Node::Leaf {
            summary: sum(&item_summaries),
            items,
            item_summaries,
        }))
    }

    /// An internal node over `children`, which must share one height. Never
    /// called with fewer than two children except through `from_children`.
    fn internal(children: Vec<SumTree<T>>) -> Self {
        let height = children.first().map_or(0, |child| child.height()) + 1;
        let child_summaries: Vec<T::Summary> = children
            .iter()
            .map(|child| child.summary().clone())
            .collect();
        Self(Arc::new(Node::Internal {
            height,
            summary: sum(&child_summaries),
            children,
            child_summaries,
        }))
    }

    /// The tree formed by consecutive siblings, which may be fewer than
    /// `MIN_CHILDREN` because the result is a root.
    fn from_children(mut children: Vec<SumTree<T>>) -> Self {
        match children.len() {
            0 => Self::new(),
            1 => children.pop().unwrap_or_default(),
            _ => Self::internal(children),
        }
    }

    /// The summary of every item.
    pub fn summary(&self) -> &T::Summary {
        self.0.summary()
    }

    /// The total extent of the tree in dimension `D`.
    pub fn extent<D: Dimension<T::Summary>>(&self) -> D {
        D::from_summary(self.summary())
    }

    /// Whether the tree has no items.
    pub fn is_empty(&self) -> bool {
        matches!(&*self.0, Node::Leaf { items, .. } if items.is_empty())
    }

    fn height(&self) -> u8 {
        self.0.height()
    }

    /// Iterates over the items in order.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            cursor: self.cursor::<()>(),
        }
    }

    /// A cursor on the first item, tracking positions in dimension `D`.
    pub fn cursor<D: Dimension<T::Summary>>(&self) -> Cursor<'_, T, D> {
        let mut cursor = Cursor {
            tree: self,
            stack: Vec::with_capacity(usize::from(self.height()) + 1),
            position: D::default(),
        };
        cursor.descend_first(&self.0, D::default());
        cursor
    }

    /// Appends one item. O(log n).
    pub fn push(&mut self, item: T) {
        self.append(Self::from_item(item));
    }

    /// Appends every item of `other`. O(log n): only the edge of the taller
    /// tree is copied.
    pub fn append(&mut self, other: Self) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            *self = other;
            return;
        }
        if self.height() >= other.height() {
            if let Some(right) = self.push_right(other) {
                let left = std::mem::take(self);
                *self = Self::internal(vec![left, right]);
            }
        } else {
            let mut tree = other;
            if let Some(left) = tree.push_left(std::mem::take(self)) {
                tree = Self::internal(vec![left, tree]);
            }
            *self = tree;
        }
    }

    /// Appends `tree`, which is not taller than `self`. Returns a new right
    /// sibling of `self`'s height if `self` overflowed.
    fn push_right(&mut self, tree: Self) -> Option<Self> {
        let tree_summary = tree.summary().clone();
        let tree_height = tree.height();
        let node = Arc::make_mut(&mut self.0);
        if node.height() == tree_height {
            merge_nodes(node, Arc::unwrap_or_clone(tree.0));
        } else if let Node::Internal {
            summary,
            children,
            child_summaries,
            ..
        } = node
        {
            let last = children.len() - 1;
            let overflow = children[last].push_right(tree);
            child_summaries[last] = children[last].summary().clone();
            if let Some(sibling) = overflow {
                child_summaries.push(sibling.summary().clone());
                children.push(sibling);
            }
            summary.add_summary(&tree_summary);
        }
        split_overflow(node, false)
    }

    /// Prepends `tree`, which is shorter than `self`. Returns a new left
    /// sibling of `self`'s height if `self` overflowed.
    fn push_left(&mut self, tree: Self) -> Option<Self> {
        let tree_height = tree.height();
        let node = Arc::make_mut(&mut self.0);
        if node.height() == tree_height {
            let mut merged = Arc::unwrap_or_clone(tree.0);
            merge_nodes(&mut merged, std::mem::replace(node, empty_leaf()));
            *node = merged;
        } else if let Node::Internal {
            summary,
            children,
            child_summaries,
            ..
        } = node
        {
            let mut new_summary = tree.summary().clone();
            new_summary.add_summary(summary);
            *summary = new_summary;
            let overflow = children[0].push_left(tree);
            child_summaries[0] = children[0].summary().clone();
            if let Some(sibling) = overflow {
                child_summaries.insert(0, sibling.summary().clone());
                children.insert(0, sibling);
            }
        }
        split_overflow(node, true)
    }

    /// Splits the tree where a seek for `target` with `bias` stops (see
    /// [`Cursor::seek`]): the left tree holds the items before that item, the
    /// right tree that item and the rest. O(log n) nodes are copied.
    pub fn split<D>(&self, target: &D, bias: Bias) -> (Self, Self)
    where
        D: Dimension<T::Summary> + Ord,
    {
        let mut position = D::default();
        self.split_at(target, bias, &mut position)
    }

    fn split_at<D, Tg>(&self, target: &Tg, bias: Bias, position: &mut D) -> (Self, Self)
    where
        D: Dimension<T::Summary>,
        Tg: SeekTarget<T::Summary, D> + ?Sized,
    {
        match &*self.0 {
            Node::Leaf {
                items,
                item_summaries,
                ..
            } => {
                let index = seek_index(item_summaries, target, bias, position);
                (
                    Self::leaf_from_parts(
                        items[..index].to_vec(),
                        item_summaries[..index].to_vec(),
                    ),
                    Self::leaf_from_parts(
                        items[index..].to_vec(),
                        item_summaries[index..].to_vec(),
                    ),
                )
            }
            Node::Internal {
                children,
                child_summaries,
                ..
            } => {
                let index = seek_index(child_summaries, target, bias, position);
                let Some(child) = children.get(index) else {
                    return (self.clone(), Self::new());
                };
                let (child_left, child_right) = child.split_at(target, bias, position);
                let mut left = Self::from_children(children[..index].to_vec());
                left.append(child_left);
                let mut right = child_right;
                right.append(Self::from_children(children[index + 1..].to_vec()));
                (left, right)
            }
        }
    }
}

#[cfg(test)]
impl<T: Item> SumTree<T> {
    /// Checks the B-tree invariants and cached summaries. Returns the height.
    pub(crate) fn check_invariants(&self) -> u8
    where
        T::Summary: PartialEq + fmt::Debug,
    {
        fn check<T: Item>(tree: &SumTree<T>, is_root: bool) -> u8
        where
            T::Summary: PartialEq + fmt::Debug,
        {
            let node = &*tree.0;
            let len = node.len();
            assert!(len <= MAX_CHILDREN, "node overflow: {len}");
            if !is_root {
                assert!(len >= MIN_CHILDREN, "node underflow: {len}");
            }
            assert_eq!(&sum(node.summaries()), node.summary(), "stale summary");
            match node {
                Node::Leaf {
                    items,
                    item_summaries,
                    ..
                } => {
                    for (item, summary) in items.iter().zip(item_summaries) {
                        assert_eq!(&item.summary(), summary, "stale item summary");
                    }
                    0
                }
                Node::Internal {
                    height,
                    children,
                    child_summaries,
                    ..
                } => {
                    assert!(!is_root || len >= 2, "root with one child");
                    for (child, summary) in children.iter().zip(child_summaries) {
                        assert_eq!(child.summary(), summary, "stale child summary");
                        assert_eq!(check(child, false) + 1, *height, "unbalanced");
                    }
                    *height
                }
            }
        }
        check(self, true)
    }
}

fn empty_leaf<T: Item>() -> Node<T> {
    Node::Leaf {
        summary: T::Summary::default(),
        items: Vec::new(),
        item_summaries: Vec::new(),
    }
}

/// Appends the entries of `right` to `left`. Both have the same height.
fn merge_nodes<T: Item>(left: &mut Node<T>, right: Node<T>) {
    match (left, right) {
        (
            Node::Leaf {
                summary,
                items,
                item_summaries,
            },
            Node::Leaf {
                summary: right_summary,
                items: right_items,
                item_summaries: right_summaries,
            },
        ) => {
            summary.add_summary(&right_summary);
            items.extend(right_items);
            item_summaries.extend(right_summaries);
        }
        (
            Node::Internal {
                summary,
                children,
                child_summaries,
                ..
            },
            Node::Internal {
                summary: right_summary,
                children: right_children,
                child_summaries: right_summaries,
                ..
            },
        ) => {
            summary.add_summary(&right_summary);
            children.extend(right_children);
            child_summaries.extend(right_summaries);
        }
        // Equal heights mean equal node kinds.
        _ => unreachable!("merging nodes of different heights"),
    }
}

/// Splits a node with more than `MAX_CHILDREN` entries into two halves of at
/// least `MIN_CHILDREN` each. `node` keeps the right half if `keep_right`,
/// and the other half is returned.
fn split_overflow<T: Item>(node: &mut Node<T>, keep_right: bool) -> Option<SumTree<T>> {
    let len = node.len();
    if len <= MAX_CHILDREN {
        return None;
    }
    let mid = len / 2;
    let (kept_summary, split_off) = match node {
        Node::Leaf {
            items,
            item_summaries,
            ..
        } => {
            let mut tail_items = items.split_off(mid);
            let mut tail_summaries = item_summaries.split_off(mid);
            if keep_right {
                std::mem::swap(items, &mut tail_items);
                std::mem::swap(item_summaries, &mut tail_summaries);
            }
            (
                sum(item_summaries),
                SumTree::leaf_from_parts(tail_items, tail_summaries),
            )
        }
        Node::Internal {
            children,
            child_summaries,
            ..
        } => {
            let mut tail_children = children.split_off(mid);
            let mut tail_summaries = child_summaries.split_off(mid);
            if keep_right {
                std::mem::swap(children, &mut tail_children);
                std::mem::swap(child_summaries, &mut tail_summaries);
            }
            (sum(child_summaries), SumTree::internal(tail_children))
        }
    };
    match node {
        Node::Leaf { summary, .. } | Node::Internal { summary, .. } => *summary = kept_summary,
    }
    Some(split_off)
}

/// The index of the first entry a seek stops in, advancing `position` past
/// the entries before it. Returns `summaries.len()` if the seek passes them
/// all.
fn seek_index<S, D, Tg>(summaries: &[S], target: &Tg, bias: Bias, position: &mut D) -> usize
where
    S: Summary,
    D: Dimension<S>,
    Tg: SeekTarget<S, D> + ?Sized,
{
    for (index, summary) in summaries.iter().enumerate() {
        let mut end = position.clone();
        end.add_summary(summary);
        if stops_before(target, &end, bias) {
            return index;
        }
        *position = end;
    }
    summaries.len()
}

/// A position in a [`SumTree`]: one item, or the end. Tracks the position in
/// dimension `D` at the start of the current item.
///
/// Seeking costs O(log n). Moving to the next or previous item costs O(1)
/// amortized.
pub struct Cursor<'a, T: Item, D> {
    tree: &'a SumTree<T>,
    /// The path from the root to the current leaf. Empty at the end.
    stack: Vec<Frame<'a, T, D>>,
    /// The position at the start of the current item, or the extent of the
    /// tree at the end.
    position: D,
}

struct Frame<'a, T: Item, D> {
    node: &'a Node<T>,
    index: usize,
    /// The position at the start of `node`.
    start: D,
}

impl<'a, T: Item, D: Dimension<T::Summary>> Cursor<'a, T, D> {
    /// The current item, or `None` at the end.
    pub fn item(&self) -> Option<&'a T> {
        let frame = self.stack.last()?;
        match frame.node {
            Node::Leaf { items, .. } => items.get(frame.index),
            Node::Internal { .. } => None,
        }
    }

    /// The summary of the current item, or `None` at the end.
    pub fn item_summary(&self) -> Option<&'a T::Summary> {
        let frame = self.stack.last()?;
        frame.node.summaries().get(frame.index)
    }

    /// The position at the start of the current item, or the extent of the
    /// tree at the end.
    pub fn start(&self) -> &D {
        &self.position
    }

    /// The position at the end of the current item, or the extent of the
    /// tree at the end.
    pub fn end(&self) -> D {
        let mut end = self.position.clone();
        if let Some(summary) = self.item_summary() {
            end.add_summary(summary);
        }
        end
    }

    /// Moves to the item that contains `target`: the first item whose end is
    /// after `target`. If `target` is exactly the end of an item, `Bias::Left`
    /// stops in that item and `Bias::Right` in the next one. Moves to the end
    /// if `target` is at or past the extent of the tree (with `Bias::Right`).
    pub fn seek<Tg>(&mut self, target: &Tg, bias: Bias)
    where
        Tg: SeekTarget<T::Summary, D> + ?Sized,
    {
        self.stack.clear();
        self.position = D::default();
        let mut node = &*self.tree.0;
        loop {
            let start = self.position.clone();
            let index = seek_index(node.summaries(), target, bias, &mut self.position);
            if index == node.len() {
                // Past every entry; `position` is already the end.
                self.stack.clear();
                return;
            }
            self.stack.push(Frame { node, index, start });
            match node {
                Node::Leaf { .. } => return,
                Node::Internal { children, .. } => node = &children[index].0,
            }
        }
    }

    /// Moves to the next item, or to the end. Does nothing at the end.
    pub fn next(&mut self) {
        let Some(summary) = self.item_summary() else {
            return;
        };
        self.position.add_summary(summary);
        while let Some(frame) = self.stack.last_mut() {
            frame.index += 1;
            if frame.index < frame.node.len() {
                if let Node::Internal { children, .. } = frame.node {
                    let child = &children[frame.index].0;
                    let start = self.position.clone();
                    self.descend_first(child, start);
                }
                return;
            }
            self.stack.pop();
        }
    }

    /// Moves to the previous item. Returns false, without moving, on the
    /// first item or in an empty tree. From the end, moves to the last item.
    pub fn prev(&mut self) -> bool {
        if self.stack.is_empty() {
            if self.tree.is_empty() {
                return false;
            }
            self.descend_last(&self.tree.0, D::default());
            return true;
        }
        let Some(depth) = self.stack.iter().rposition(|frame| frame.index > 0) else {
            return false;
        };
        self.stack.truncate(depth + 1);
        let frame = &mut self.stack[depth];
        frame.index -= 1;
        let (node, index) = (frame.node, frame.index);
        let mut position = frame.start.clone();
        for summary in &node.summaries()[..index] {
            position.add_summary(summary);
        }
        match node {
            Node::Leaf { .. } => self.position = position,
            Node::Internal { children, .. } => self.descend_last(&children[index].0, position),
        }
        true
    }

    /// Pushes frames down the leftmost path of `node`, which starts at
    /// `start`.
    fn descend_first(&mut self, mut node: &'a Node<T>, start: D) {
        if node.len() == 0 {
            self.position = start;
            return;
        }
        loop {
            self.stack.push(Frame {
                node,
                index: 0,
                start: start.clone(),
            });
            match node {
                Node::Leaf { .. } => break,
                Node::Internal { children, .. } => node = &children[0].0,
            }
        }
        self.position = start;
    }

    /// Pushes frames down the rightmost path of `node`, which starts at
    /// `start`.
    fn descend_last(&mut self, mut node: &'a Node<T>, mut start: D) {
        loop {
            let summaries = node.summaries();
            let index = summaries.len().saturating_sub(1);
            let mut position = start.clone();
            for summary in &summaries[..index] {
                position.add_summary(summary);
            }
            self.stack.push(Frame { node, index, start });
            match node {
                Node::Leaf { .. } => {
                    self.position = position;
                    return;
                }
                Node::Internal { children, .. } => {
                    node = &children[index].0;
                    start = position;
                }
            }
        }
    }
}

/// Iterates over the items of a [`SumTree`] in order.
pub struct Iter<'a, T: Item> {
    cursor: Cursor<'a, T, ()>,
}

impl<'a, T: Item> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        let item = self.cursor.item()?;
        self.cursor.next();
        Some(item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Test items are numbers; the summary counts them and adds them up.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Num(u32);

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct NumSummary {
        count: usize,
        sum: u64,
    }

    impl Summary for NumSummary {
        fn add_summary(&mut self, other: &Self) {
            self.count += other.count;
            self.sum += other.sum;
        }
    }

    impl Item for Num {
        type Summary = NumSummary;
        fn summary(&self) -> NumSummary {
            NumSummary {
                count: 1,
                sum: u64::from(self.0),
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
    struct Count(usize);

    impl Dimension<NumSummary> for Count {
        fn add_summary(&mut self, summary: &NumSummary) {
            self.0 += summary.count;
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
    struct Sum(u64);

    impl Dimension<NumSummary> for Sum {
        fn add_summary(&mut self, summary: &NumSummary) {
            self.0 += summary.sum;
        }
    }

    fn tree_of(values: &[u32]) -> SumTree<Num> {
        values.iter().copied().map(Num).collect()
    }

    fn contents(tree: &SumTree<Num>) -> Vec<u32> {
        tree.iter().map(|num| num.0).collect()
    }

    fn naive_summary(values: &[u32]) -> NumSummary {
        NumSummary {
            count: values.len(),
            sum: values.iter().copied().map(u64::from).sum(),
        }
    }

    /// The index a seek by running sum stops at, computed by a scan.
    fn naive_seek_by_sum(values: &[u32], target: u64, bias: Bias) -> usize {
        let mut end = 0;
        for (index, value) in values.iter().enumerate() {
            end += u64::from(*value);
            let stops = match target.cmp(&end) {
                Ordering::Less => true,
                Ordering::Equal => bias == Bias::Left,
                Ordering::Greater => false,
            };
            if stops {
                return index;
            }
        }
        values.len()
    }

    #[test]
    fn empty_tree() {
        let tree = SumTree::<Num>::new();
        assert!(tree.is_empty());
        assert_eq!(tree.summary(), &NumSummary::default());
        assert_eq!(tree.iter().count(), 0);
        let mut cursor = tree.cursor::<Count>();
        assert!(cursor.item().is_none());
        assert!(!cursor.prev());
        cursor.next();
        cursor.seek(&Count(3), Bias::Right);
        assert!(cursor.item().is_none());
        let (left, right) = tree.split(&Count(0), Bias::Right);
        assert!(left.is_empty() && right.is_empty());
        tree.check_invariants();
    }

    #[test]
    fn clone_shares_nodes_and_is_independent() {
        let mut tree = tree_of(&(0..100).collect::<Vec<_>>());
        let snapshot = tree.clone();
        assert!(Arc::ptr_eq(&tree.0, &snapshot.0));
        tree.push(Num(7));
        assert_eq!(snapshot.summary().count, 100);
        assert_eq!(tree.summary().count, 101);
    }

    #[test]
    fn snapshots_are_send_and_sync() {
        fn assert_send_sync<X: Send + Sync>() {}
        assert_send_sync::<SumTree<Num>>();
    }

    #[test]
    fn seek_bias_at_item_boundaries() {
        let tree = tree_of(&[3, 0, 4]);
        let mut cursor = tree.cursor::<Sum>();
        cursor.seek(&Sum(3), Bias::Left);
        assert_eq!(cursor.item(), Some(&Num(3)));
        cursor.seek(&Sum(3), Bias::Right);
        assert_eq!(cursor.item(), Some(&Num(4)), "the empty item is skipped");
        assert_eq!(cursor.start(), &Sum(3));
        cursor.seek(&Sum(7), Bias::Left);
        assert_eq!(cursor.item(), Some(&Num(4)));
        cursor.seek(&Sum(7), Bias::Right);
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.start(), &Sum(7));
    }

    #[test]
    fn seek_by_tuple_dimension_reads_the_second() {
        let tree = tree_of(&[5, 6, 7, 8]);
        let mut cursor = tree.cursor::<(Count, Sum)>();
        cursor.seek(&Count(2), Bias::Right);
        assert_eq!(cursor.start(), &(Count(2), Sum(11)));
        assert_eq!(cursor.end(), (Count(3), Sum(18)));
    }

    /// What `replace_items` must leave: two splits and two appends.
    fn replaced_by_splits<D>(
        tree: &SumTree<Num>,
        range: std::ops::Range<D>,
        items: &[u32],
    ) -> Vec<u32>
    where
        D: Dimension<NumSummary> + Ord,
    {
        let (mut left, _) = tree.split(&range.start, Bias::Right);
        let (_, right) = tree.split(&range.end, Bias::Right);
        left.append(tree_of(items));
        left.append(right);
        contents(&left)
    }

    /// The summary of what splits at both ends of `range` cut out.
    fn summary_by_splits<D>(tree: &SumTree<Num>, range: &std::ops::Range<D>) -> NumSummary
    where
        D: Dimension<NumSummary> + Ord,
    {
        let (before_end, _) = tree.split(&range.end, Bias::Right);
        let (_, between) = before_end.split(&range.start, Bias::Right);
        *between.summary()
    }

    proptest! {
        /// The summary of a range, read in place, is what splitting would cut out, by count
        /// and by a sum that items of no size share (ADR 0101).
        #[test]
        fn a_ranges_summary_is_what_splits_cut_out(
            values in prop::collection::vec(0u32..4, 0..120),
            a in 0usize..140,
            b in 0usize..140,
        ) {
            let tree = tree_of(&values);
            let (start, end) = (a.min(b), a.max(b));
            let by_count = Count(start)..Count(end);
            prop_assert_eq!(tree.summary_in(&by_count), summary_by_splits(&tree, &by_count));
            let by_sum = Sum(start as u64)..Sum(end as u64);
            prop_assert_eq!(tree.summary_in(&by_sum), summary_by_splits(&tree, &by_sum));
        }
    }

    proptest! {
        /// Replacing in place, by count or by a sum that many items share, leaves what
        /// splitting and appending would, a valid tree, and the trees it was cloned from
        /// as they were (ADR 0091).
        #[test]
        fn replacing_items_is_splitting_and_appending(
            values in prop::collection::vec(0u32..4, 0..120),
            edits in prop::collection::vec(
                (any::<bool>(), 0usize..140, 0usize..140, prop::collection::vec(0u32..4, 0..12)),
                1..20,
            ),
        ) {
            let mut tree = tree_of(&values);
            for (by_count, a, b, items) in edits {
                let before = tree.clone();
                let kept = contents(&before);
                let (start, end) = (a.min(b), a.max(b));
                let expected = if by_count {
                    replaced_by_splits(&tree, Count(start)..Count(end), &items)
                } else {
                    replaced_by_splits(&tree, Sum(start as u64)..Sum(end as u64), &items)
                };
                let items = items.into_iter().map(Num).collect();
                if by_count {
                    tree.replace_items(Count(start)..Count(end), items);
                } else {
                    tree.replace_items(Sum(start as u64)..Sum(end as u64), items);
                }
                tree.check_invariants();
                prop_assert_eq!(contents(&tree), expected);
                prop_assert_eq!(contents(&before), kept);
            }
        }

        /// Edits in place one after another, as a command's at many cursors, leave what
        /// `Vec::splice` would and a valid tree, and a clone taken between them keeps what
        /// it held (ADR 0091).
        #[test]
        fn replacing_in_place_again_and_again_is_splicing(
            values in prop::collection::vec(0u32..10, 0..200),
            edits in prop::collection::vec(
                (0usize..220, 0usize..6, prop::collection::vec(0u32..10, 0..20)),
                1..40,
            ),
            snapshot_at in 0usize..40,
        ) {
            let mut tree = tree_of(&values);
            let mut model = values;
            let mut snapshot = None;
            for (index, (at, len, items)) in edits.into_iter().enumerate() {
                if index == snapshot_at {
                    snapshot = Some((tree.clone(), model.clone()));
                }
                let start = at.min(model.len());
                let end = (start + len).min(model.len());
                model.splice(start..end, items.iter().copied());
                tree.replace_items(Count(start)..Count(end), items.into_iter().map(Num).collect());
                tree.check_invariants();
                prop_assert_eq!(contents(&tree), model.clone());
            }
            if let Some((tree, model)) = snapshot {
                tree.check_invariants();
                prop_assert_eq!(contents(&tree), model);
            }
        }

        #[test]
        fn building_matches_the_input(values in prop::collection::vec(0u32..10, 0..300)) {
            let tree = tree_of(&values);
            tree.check_invariants();
            prop_assert_eq!(&contents(&tree), &values);
            prop_assert_eq!(tree.summary(), &naive_summary(&values));

            let mut pushed = SumTree::new();
            for value in &values {
                pushed.push(Num(*value));
            }
            pushed.check_invariants();
            prop_assert_eq!(contents(&pushed), values);
        }

        #[test]
        fn append_concatenates(
            a in prop::collection::vec(0u32..10, 0..200),
            b in prop::collection::vec(0u32..10, 0..200),
        ) {
            let mut tree = tree_of(&a);
            let other = tree_of(&b);
            let before = tree.clone();
            tree.append(other.clone());
            tree.check_invariants();
            let mut expected = a.clone();
            expected.extend(&b);
            prop_assert_eq!(contents(&tree), expected);
            // The inputs are untouched snapshots.
            prop_assert_eq!(contents(&before), a);
            prop_assert_eq!(contents(&other), b);
        }

        #[test]
        fn split_and_rejoin(
            values in prop::collection::vec(0u32..5, 0..300),
            target in 0u64..700,
            right_bias in any::<bool>(),
        ) {
            let bias = if right_bias { Bias::Right } else { Bias::Left };
            let tree = tree_of(&values);
            let (left, right) = tree.split(&Sum(target), bias);
            left.check_invariants();
            right.check_invariants();
            let index = naive_seek_by_sum(&values, target, bias);
            prop_assert_eq!(contents(&left), values[..index].to_vec());
            prop_assert_eq!(contents(&right), values[index..].to_vec());
            let mut joined = left;
            joined.append(right);
            joined.check_invariants();
            prop_assert_eq!(contents(&joined), values);
        }

        #[test]
        fn random_splices_keep_the_tree_valid(
            values in prop::collection::vec(0u32..5, 0..200),
            ops in prop::collection::vec((0usize..250, 0usize..250, prop::collection::vec(0u32..5, 0..40)), 1..20),
        ) {
            let mut model = values.clone();
            let mut tree = tree_of(&values);
            for (a, b, insert) in ops {
                let (start, end) = (a.min(b).min(model.len()), a.max(b).min(model.len()));
                let (left, rest) = tree.split(&Count(start), Bias::Right);
                let (_, right) = rest.split(&Count(end - start), Bias::Right);
                tree = left;
                tree.append(tree_of(&insert));
                tree.append(right);
                model.splice(start..end, insert);
                tree.check_invariants();
                prop_assert_eq!(contents(&tree), model.clone());
            }
        }

        #[test]
        fn cursor_walks_both_ways(values in prop::collection::vec(0u32..5, 0..200), seek in 0usize..220) {
            let tree = tree_of(&values);
            let mut cursor = tree.cursor::<(Count, Sum)>();
            let mut seen = Vec::new();
            while let Some(item) = cursor.item() {
                let (count, sum) = *cursor.start();
                prop_assert_eq!(count.0, seen.len());
                prop_assert_eq!(sum.0, seen.iter().copied().map(u64::from).sum::<u64>());
                seen.push(item.0);
                cursor.next();
            }
            prop_assert_eq!(&seen, &values);
            prop_assert_eq!(cursor.start().0, Count(values.len()));

            // Walk back from the end.
            let mut back = Vec::new();
            while cursor.prev() {
                let item = cursor.item().map(|num| num.0);
                prop_assert_eq!(cursor.start().0, Count(values.len() - back.len() - 1));
                back.extend(item);
            }
            back.reverse();
            prop_assert_eq!(&back, &values);

            // Seek, then step back and forth.
            cursor.seek(&Count(seek), Bias::Right);
            let index = seek.min(values.len());
            prop_assert_eq!(cursor.item().map(|num| num.0), values.get(index).copied());
            prop_assert_eq!(cursor.start().0, Count(index));
            if cursor.prev() {
                prop_assert_eq!(cursor.start().0, Count(index - 1));
                prop_assert_eq!(cursor.item().map(|num| num.0), values.get(index - 1).copied());
                cursor.next();
                prop_assert_eq!(cursor.start().0, Count(index));
            } else {
                prop_assert_eq!(index, 0);
            }
        }
    }
}
