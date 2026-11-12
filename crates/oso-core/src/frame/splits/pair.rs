//! A pair of windows in the tree (ADR 0122, step 10): two windows that scroll together, as
//! one run of exactly two, which the tree splits, resizes and deletes as one.
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type. The tree knows
//! which windows are paired, and lays them out; the editor keeps the row map between them and
//! scrolls them.
//!
//! A pair's windows have the same weight, so they share its cells, the first taking the larger
//! half when they are odd. A pair is never merged into a run of its way, so a pair side by
//! side sits in a run side by side as one child. When its run cannot give it what both its
//! windows need, neither shows, unless the pair holds the window kept shown, or is what is
//! left to show in an area smaller than both need, where its first window shows alone, as a
//! run's first does.

use super::{Child, FrameLayout, Node, Run, SPLIT_COLUMNS, SPLIT_ROWS, Splits, TooSmall, Way};
use crate::window::WindowId;

impl Splits {
    /// Makes the window `at` a pair with `new`, `at` above or at the left and `new` below or
    /// at the right, sharing its cells, the smallest a split leaves checked as
    /// [`Splits::split`] checks it.
    pub(crate) fn pair(
        &mut self,
        at: WindowId,
        way: Way,
        new: WindowId,
        layout: &FrameLayout,
    ) -> Result<(), TooSmall> {
        let unit = self.root.unit(at).ok_or(TooSmall)?;
        if *unit != Node::Window(at) || self.contains(new) {
            return Err(TooSmall);
        }
        self.halves(unit, way, layout)?;
        if let Some(node) = self.root.unit_mut(at) {
            *node = Node::Run(Run {
                way,
                children: vec![
                    Child {
                        weight: 1,
                        node: Node::Window(at),
                    },
                    Child {
                        weight: 1,
                        node: Node::Window(new),
                    },
                ],
                pair: true,
            });
        }
        Ok(())
    }

    /// The other window of the pair `window` is in.
    pub(crate) fn pair_of(&self, window: WindowId) -> Option<WindowId> {
        match self.root.unit(window)? {
            Node::Run(run) if run.pair => run.children.iter().find_map(|c| match c.node {
                Node::Window(id) if id != window => Some(id),
                _ => None,
            }),
            _ => None,
        }
    }

    /// Lays the pair that `window` is in out `way`, its windows in the same order. Returns
    /// whether it changed: not for a window of no pair, nor a pair laid out so already.
    ///
    /// # Errors
    ///
    /// [`TooSmall`] when either window would keep less than a split leaves. Nothing changes.
    pub(crate) fn turn(
        &mut self,
        window: WindowId,
        way: Way,
        layout: &FrameLayout,
    ) -> Result<bool, TooSmall> {
        let gutter = self.gutter;
        let Some(Node::Run(run)) = self.root.unit_mut(window) else {
            return Ok(false);
        };
        if !run.pair || run.way == way {
            return Ok(false);
        }
        let unit = Node::Run(run.clone());
        let divider = unit
            .last_window()
            .and_then(|last| layout.regions(last))
            .is_some_and(|regions| regions.divider.is_some());
        let columns = gutter + SPLIT_COLUMNS;
        let rows = SPLIT_ROWS + 1;
        // Both windows along `way`, each with a divider but at the right edge as it is.
        let (need_rows, need_columns) = match way {
            Way::Across => (rows, 2 * columns + 1 + usize::from(divider)),
            Way::Down => (2 * rows, columns + usize::from(divider)),
        };
        if unit.extent(Way::Down, layout) < need_rows
            || unit.extent(Way::Across, layout) < need_columns
        {
            return Err(TooSmall);
        }
        run.way = way;
        Ok(true)
    }

    /// Takes the window paired with `keep` out of the tree, `keep` taking the pair's cells as a
    /// window of its own. Returns the window taken out, or `None` for a window of no pair.
    pub(crate) fn unpair(&mut self, keep: WindowId) -> Option<WindowId> {
        let other = self.pair_of(keep)?;
        let node = self.root.unit_mut(keep)?;
        *node = Node::Window(keep);
        Some(other)
    }

    /// Makes the pair `window` is in two windows of their own, laid out as they are, merged
    /// into the run around them when it runs their way. Returns whether there was a pair.
    pub(crate) fn break_pair(&mut self, window: WindowId, layout: &FrameLayout) -> bool {
        if self.pair_of(window).is_none() {
            return false;
        }
        if let Node::Run(run) = &mut self.root
            && run.pair
        {
            run.pair = false;
            return true;
        }
        self.root.break_pair(window, layout)
    }

    /// A frame of the window `keep` alone, or of its pair when it is in one, as
    /// `delete-other-windows` leaves it (ADR 0122).
    pub(crate) fn only(&self, keep: WindowId) -> Self {
        let root = match self.root.unit(keep) {
            Some(Node::Run(run)) if run.pair => Node::Run(run.clone()),
            _ => Node::Window(keep),
        };
        Self {
            root,
            gutter: self.gutter,
        }
    }

    /// How the pair `window` is in is laid out.
    #[cfg(test)]
    pub(crate) fn pair_way(&self, window: WindowId) -> Option<Way> {
        match self.root.unit(window)? {
            Node::Run(run) if run.pair => Some(run.way),
            _ => None,
        }
    }

    /// Every pair, as its windows in order.
    #[cfg(test)]
    pub(crate) fn pairs(&self) -> Vec<[WindowId; 2]> {
        let mut pairs = Vec::new();
        self.root.pairs(&mut pairs);
        pairs
    }
}

impl Node {
    /// Whether it is what the tree splits, resizes and deletes for `window`: the window
    /// itself, or the pair it is in.
    pub(super) fn is_unit_of(&self, window: WindowId) -> bool {
        match self {
            Self::Window(id) => *id == window,
            Self::Run(run) => run.pair && self.contains(window),
        }
    }

    /// The window `window`, or the pair it is in, if it is in the tree.
    pub(super) fn unit(&self, window: WindowId) -> Option<&Self> {
        if self.is_unit_of(window) {
            return Some(self);
        }
        match self {
            Self::Window(_) => None,
            Self::Run(run) => run.children.iter().find_map(|c| c.node.unit(window)),
        }
    }

    fn unit_mut(&mut self, window: WindowId) -> Option<&mut Self> {
        if self.is_unit_of(window) {
            return Some(self);
        }
        match self {
            Self::Window(_) => None,
            Self::Run(run) => run
                .children
                .iter_mut()
                .find(|c| c.node.contains(window))
                .and_then(|c| c.node.unit_mut(window)),
        }
    }

    /// Its last window in the tree's order: the right-most of its bottom row.
    pub(super) fn last_window(&self) -> Option<WindowId> {
        match self {
            Self::Window(id) => Some(*id),
            Self::Run(run) => run.children.last()?.node.last_window(),
        }
    }

    /// Breaks the pair `window` is in below this run, merging it in when it runs this way.
    fn break_pair(&mut self, window: WindowId, layout: &FrameLayout) -> bool {
        let Self::Run(run) = self else {
            return false;
        };
        let Some(index) = run.children.iter().position(|c| c.node.contains(window)) else {
            return false;
        };
        let way = run.way;
        match &mut run.children[index].node {
            Self::Run(inner) if inner.pair => {
                inner.pair = false;
                if inner.way == way {
                    // In cells, as it lies across the whole of the child it replaces.
                    run.reset(layout);
                    if let Self::Run(mut inner) =
                        std::mem::replace(&mut run.children[index].node, Self::Window(window))
                    {
                        inner.reset(layout);
                        run.children.splice(index..=index, inner.children);
                    }
                }
                true
            }
            child => child.break_pair(window, layout),
        }
    }

    /// Panics on a pair that is not two windows, which the tree never makes.
    #[cfg(test)]
    fn pairs(&self, out: &mut Vec<[WindowId; 2]>) {
        if let Self::Run(run) = self {
            if run.pair {
                let [first, second] = run.children.as_slice() else {
                    panic!("a pair of {} children", run.children.len());
                };
                let (Self::Window(a), Self::Window(b)) = (&first.node, &second.node) else {
                    panic!("a pair holds a run");
                };
                out.push([*a, *b]);
            }
            run.children.iter().for_each(|c| c.node.pairs(out));
        }
    }
}

#[cfg(test)]
mod tests;
