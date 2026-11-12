//! The tree of a frame's tiled windows, and how it divides the frame's cells among them
//! (ADR 0112).
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type. The tree holds
//! window ids only; the editor keeps the windows.
//!
//! A leaf is a window. A run is two or more children laid out the same way, side by side or
//! stacked, and never has a child that is a run of the same way, but a pair of windows that
//! scroll together (ADR 0122), which stays one child. Each child keeps a weight,
//! and a layout turns the weights into whole cells by the largest remainder, so a frame that
//! is resized keeps the proportions, and resizing it back gives the same cells. A change of
//! the tree first sets the weights it touches to the cells laid out, so that it works in
//! cells and keeps what the person sees.

use super::{CellRect, FrameLayout, WindowRegions};
use crate::window::WindowId;
use cells::{distribute, window_regions};
pub use tiling::{BadTiling, Tiling};

/// The fewest text rows each half of a split keeps, above its modeline.
const SPLIT_ROWS: usize = 2;
/// The fewest text columns each half of a split keeps, beside the narrowest gutter: the
/// narrowest a window wraps at (ADR 0104).
const SPLIT_COLUMNS: usize = 10;

/// How a run lays out its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Way {
    /// Side by side, from the left.
    Across,
    /// Stacked, from the top.
    Down,
}

/// A side of a window, for the window next to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// A split refused, as a half would be smaller than a window may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooSmall;

impl std::fmt::Display for TooSmall {
    /// As Emacs says it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Window too small for splitting")
    }
}

impl std::error::Error for TooSmall {}

/// The tiled windows of a frame, as a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Splits {
    root: Node,
    /// The columns the narrowest gutter takes while line numbers are on, and 0 while they
    /// are off, which the smallest window counts in place of its own gutter (ADR 0112).
    gutter: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Window(WindowId),
    Run(Run),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Run {
    way: Way,
    children: Vec<Child>,
    /// A pair of windows that scroll together (ADR 0122): exactly two windows of the same
    /// weight, split, resized and deleted as one, and never merged into a run of its way.
    pair: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Child {
    weight: usize,
    node: Node,
}

impl Splits {
    /// A frame of one window.
    pub(crate) fn new(window: WindowId) -> Self {
        Self {
            root: Node::Window(window),
            gutter: 0,
        }
    }

    /// Counts a gutter of `columns` beside each window's 10 text columns in the smallest a
    /// split or a resize leaves: the narrowest gutter while line numbers are on, 0 while they
    /// are off.
    pub(crate) fn set_gutter(&mut self, columns: usize) {
        self.gutter = columns;
    }

    /// The windows in the tree's order: left to right, top to bottom.
    pub(crate) fn iter(&self) -> impl Iterator<Item = WindowId> + '_ {
        let mut windows = Vec::new();
        self.root.windows(&mut windows);
        windows.into_iter()
    }

    /// Whether `window` is in the tree.
    pub(crate) fn contains(&self, window: WindowId) -> bool {
        self.root.contains(window)
    }

    /// Splits `at` into two, `at` above or at the left and `new` below or at the right, as
    /// `layout` lays it out. They share its cells, `new` taking the smaller half when they
    /// are odd, and no other window changes. Refused when either half would keep fewer than
    /// 2 text rows above its modeline, or fewer than 10 text columns beside the narrowest
    /// gutter ([`Splits::set_gutter`]). A window's own gutter never counts: it grows with its
    /// buffer's line count, which a buffer that may hold secrets must not give away through
    /// where a split or a resize stops (ADR 0097). A gutter wider than the narrowest leaves
    /// fewer text columns, down to the 1 a layout keeps, and a window narrower than 10 then
    /// does not wrap (ADR 0104).
    ///
    /// A window of a pair splits the pair's area: the new window is below or at the right of
    /// the whole pair, which keeps the smallest each of its windows needs (ADR 0122).
    pub(crate) fn split(
        &mut self,
        at: WindowId,
        way: Way,
        new: WindowId,
        layout: &FrameLayout,
    ) -> Result<(), TooSmall> {
        let unit = self.root.unit(at).ok_or(TooSmall)?;
        let halves = self.halves(unit, way, layout)?;
        if self.contains(new) {
            return Err(TooSmall);
        }
        self.root.split(at, way, new, halves, layout);
        Ok(())
    }

    /// The cells `unit`, a window or a pair, and a new window beside it `way` would take,
    /// sharing its cells: refused when either would keep less than its smallest, along the
    /// split or across it.
    fn halves(&self, unit: &Node, way: Way, layout: &FrameLayout) -> Result<[usize; 2], TooSmall> {
        // Its right-most window's divider, which the new window takes over at the right.
        let last = unit.last_window().ok_or(TooSmall)?;
        let divider = layout.regions(last).ok_or(TooSmall)?.divider.is_some();
        let across = match way {
            Way::Across => Way::Down,
            Way::Down => Way::Across,
        };
        let (first_least, second_least) = match way {
            Way::Down => (unit.least(way, layout, self.gutter), SPLIT_ROWS + 1),
            Way::Across => (
                unit.least(way, layout, self.gutter) + usize::from(!divider),
                self.gutter + SPLIT_COLUMNS + usize::from(divider),
            ),
        };
        let total = unit.extent(way, layout);
        let second = total / 2;
        let first = total - second;
        let across_ok = unit.extent(across, layout) >= unit.least(across, layout, self.gutter);
        if first < first_least || second < second_least || !across_ok {
            return Err(TooSmall);
        }
        Ok([first, second])
    }

    /// Takes `window` out of the tree, its cells going to the window or run before it, or
    /// after it when it is the first. Returns whether it did: the only window stays.
    ///
    /// A window of a pair takes the pair out with it, both its windows going (ADR 0122), and
    /// a pair that is the whole frame stays, as the only window does.
    pub(crate) fn remove(&mut self, window: WindowId, layout: &FrameLayout) -> bool {
        if self.root.is_unit_of(window) {
            return false;
        }
        let Node::Run(run) = &mut self.root else {
            return false;
        };
        let removed = run.remove(window, layout);
        if run.children.len() == 1
            && let Some(child) = run.children.pop()
        {
            self.root = child.node;
        }
        removed
    }

    /// Gives every child of every run the same weight.
    pub(crate) fn balance(&mut self) {
        self.root.balance();
    }

    /// Makes `window` `cells` larger along `way`, or smaller when negative, as `layout` lays
    /// it out. It grows into the windows after it in the nearest run of that way, then those
    /// before it, and shrinks into the one after it, or before it when it is the last. None
    /// goes below the smallest a split leaves. Returns how many cells it grew, negative for
    /// how many it shrank.
    pub(crate) fn resize(
        &mut self,
        window: WindowId,
        way: Way,
        cells: isize,
        layout: &FrameLayout,
    ) -> isize {
        self.root
            .resize(window, way, cells, (layout, self.gutter))
            .unwrap_or(0)
    }

    /// Divides `area` among the windows, the regions going into `out` in the tree's order.
    /// A window whose run cannot give it its smallest gets no cells, those last in the tree
    /// first, and never `keep`.
    pub(super) fn lay_out(
        &self,
        area: CellRect,
        keep: WindowId,
        gutter: &dyn Fn(WindowId) -> usize,
        out: &mut Vec<(WindowId, WindowRegions)>,
    ) {
        self.root.lay_out(area, false, keep, gutter, out);
    }
}

impl Node {
    fn windows(&self, out: &mut Vec<WindowId>) {
        match self {
            Self::Window(id) => out.push(*id),
            Self::Run(run) => run.children.iter().for_each(|c| c.node.windows(out)),
        }
    }

    fn contains(&self, window: WindowId) -> bool {
        match self {
            Self::Window(id) => *id == window,
            Self::Run(run) => run.children.iter().any(|c| c.node.contains(window)),
        }
    }

    fn split(
        &mut self,
        at: WindowId,
        way: Way,
        new: WindowId,
        [first, second]: [usize; 2],
        layout: &FrameLayout,
    ) -> bool {
        if self.is_unit_of(at) {
            let unit = std::mem::replace(self, Self::Window(new));
            *self = Self::Run(Run {
                way,
                children: vec![
                    Child {
                        weight: first,
                        node: unit,
                    },
                    Child {
                        weight: second,
                        node: Self::Window(new),
                    },
                ],
                pair: false,
            });
            return true;
        }
        match self {
            Self::Window(_) => false,
            Self::Run(run) => {
                let Some(index) = run.children.iter().position(|c| c.node.contains(at)) else {
                    return false;
                };
                if run.way == way && run.children[index].node.is_unit_of(at) {
                    run.reset(layout);
                    run.children[index].weight = first;
                    run.children.insert(
                        index + 1,
                        Child {
                            weight: second,
                            node: Self::Window(new),
                        },
                    );
                    return true;
                }
                run.children[index]
                    .node
                    .split(at, way, new, [first, second], layout)
            }
        }
    }

    fn balance(&mut self) {
        if let Self::Run(run) = self {
            for child in &mut run.children {
                child.weight = 1;
                child.node.balance();
            }
        }
    }

    fn resize(
        &mut self,
        window: WindowId,
        way: Way,
        cells: isize,
        (layout, gutter): (&FrameLayout, usize),
    ) -> Option<isize> {
        // A pair resizes as one in the run around it, its windows kept the same (ADR 0122).
        let Self::Run(run) = self else {
            return None;
        };
        if run.pair {
            return None;
        }
        let index = run.children.iter().position(|c| c.node.contains(window))?;
        let inner = (layout, gutter);
        if let Some(moved) = run.children[index].node.resize(window, way, cells, inner) {
            return Some(moved);
        }
        (run.way == way).then(|| run.resize(index, cells, inner))
    }

    /// The cells it takes along `way` in `layout`: from the first of its windows' cells to
    /// the last. None while none of them has any.
    fn extent(&self, way: Way, layout: &FrameLayout) -> usize {
        let mut span: Option<(usize, usize)> = None;
        let mut windows = Vec::new();
        self.windows(&mut windows);
        for id in windows {
            let Some(outer) = layout.regions(id).map(WindowRegions::outer) else {
                continue;
            };
            let (start, cells) = match way {
                Way::Across => (outer.column, outer.columns),
                Way::Down => (outer.row, outer.rows),
            };
            if cells == 0 {
                continue;
            }
            let end = start + cells;
            span = Some(span.map_or((start, end), |(s, e)| (s.min(start), e.max(end))));
        }
        span.map_or(0, |(start, end)| end - start)
    }

    /// The fewest cells it can take along `way` while each window keeps what a split leaves,
    /// beside a gutter of `gutter` columns.
    fn least(&self, way: Way, layout: &FrameLayout, gutter: usize) -> usize {
        match self {
            Self::Window(id) => {
                let Some(regions) = layout.regions(*id) else {
                    return 0;
                };
                match way {
                    Way::Down => SPLIT_ROWS + 1,
                    Way::Across => gutter + SPLIT_COLUMNS + usize::from(regions.divider.is_some()),
                }
            }
            Self::Run(run) => {
                let each = run
                    .children
                    .iter()
                    .map(|c| c.node.least(way, layout, gutter));
                if run.way == way {
                    each.sum()
                } else {
                    each.max().unwrap_or(0)
                }
            }
        }
    }

    /// The fewest cells it needs along `way` for each window to show: a row of text and its
    /// modeline, a column of text and its divider.
    fn needs(&self, way: Way, right: bool) -> usize {
        match self {
            Self::Window(_) => match way {
                Way::Down => 2,
                Way::Across => 1 + usize::from(right),
            },
            Self::Run(run) => {
                let last = run.children.len().saturating_sub(1);
                let each = run.children.iter().enumerate().map(|(i, c)| {
                    let right = right || (run.way == Way::Across && i < last);
                    c.node.needs(way, right)
                });
                if run.way == way {
                    each.sum()
                } else {
                    each.max().unwrap_or(0)
                }
            }
        }
    }

    fn lay_out(
        &self,
        area: CellRect,
        right: bool,
        keep: WindowId,
        gutter: &dyn Fn(WindowId) -> usize,
        out: &mut Vec<(WindowId, WindowRegions)>,
    ) {
        match self {
            Self::Window(id) => out.push((*id, window_regions(area, right, gutter(*id)))),
            Self::Run(run) => {
                let last = run.children.len().saturating_sub(1);
                let needs_right = |i: usize| right || (run.way == Way::Across && i < last);
                let total = match run.way {
                    Way::Across => area.columns,
                    Way::Down => area.rows,
                };
                let weights: Vec<usize> = run.children.iter().map(|c| c.weight).collect();
                let needs: Vec<usize> = run
                    .children
                    .iter()
                    .enumerate()
                    .map(|(i, c)| c.node.needs(run.way, needs_right(i)))
                    .collect();
                // With `keep` elsewhere, the first child, so the area always has a window.
                let kept = run
                    .children
                    .iter()
                    .position(|c| c.node.contains(keep))
                    .unwrap_or(0);
                let cells = distribute(total, &weights, &needs, kept);
                // A window has a divider only while a window shows to its right.
                let last_shown = cells.iter().rposition(|&c| c > 0).unwrap_or(0);
                let rights = |i: usize| right || (run.way == Way::Across && i < last_shown);
                let mut start = 0;
                for (i, (child, cells)) in run.children.iter().zip(cells).enumerate() {
                    let part = match run.way {
                        Way::Across => CellRect {
                            column: area.column + start,
                            columns: cells,
                            ..area
                        },
                        Way::Down => CellRect {
                            row: area.row + start,
                            rows: cells,
                            ..area
                        },
                    };
                    child.node.lay_out(part, rights(i), keep, gutter, out);
                    start += cells;
                }
            }
        }
    }
}

impl Run {
    /// Sets each child's weight to its cells along the run in `layout`, at least 1, so a
    /// change works in the cells the person sees.
    fn reset(&mut self, layout: &FrameLayout) {
        for child in &mut self.children {
            child.weight = child.node.extent(self.way, layout).max(1);
        }
    }

    fn remove(&mut self, window: WindowId, layout: &FrameLayout) -> bool {
        let Some(index) = self.children.iter().position(|c| c.node.contains(window)) else {
            return false;
        };
        if self.children[index].node.is_unit_of(window) {
            self.reset(layout);
            let into = if index > 0 { index - 1 } else { 1 };
            self.children[into].weight += self.children[index].weight;
            self.children.remove(index);
            return true;
        }
        // Its cells as laid out, before the child loses a window.
        self.reset(layout);
        let Node::Run(run) = &mut self.children[index].node else {
            return false;
        };
        let removed = run.remove(window, layout);
        if run.children.len() == 1
            && let Some(lone) = run.children.pop()
        {
            match lone.node {
                // A run of this run's way merges into it, in cells, as it lies across the
                // whole of the child it replaces.
                Node::Run(mut inner) if inner.way == self.way && !inner.pair => {
                    inner.reset(layout);
                    self.children.splice(index..=index, inner.children);
                }
                node => self.children[index].node = node,
            }
        }
        removed
    }

    fn resize(
        &mut self,
        index: usize,
        cells: isize,
        (layout, gutter): (&FrameLayout, usize),
    ) -> isize {
        self.reset(layout);
        let least: Vec<usize> = self
            .children
            .iter()
            .map(|c| c.node.least(self.way, layout, gutter))
            .collect();
        let spare = |child: &Child, least: usize| child.weight.saturating_sub(least);
        if cells >= 0 {
            let mut wanted = cells.unsigned_abs();
            let order = (index + 1..self.children.len()).chain((0..index).rev());
            for other in order {
                let given = wanted.min(spare(&self.children[other], least[other]));
                self.children[other].weight -= given;
                wanted -= given;
            }
            let grown = cells.unsigned_abs() - wanted;
            self.children[index].weight += grown;
            grown as isize
        } else {
            let Some(into) = (index + 1 < self.children.len())
                .then_some(index + 1)
                .or_else(|| index.checked_sub(1))
            else {
                return 0;
            };
            let given = cells
                .unsigned_abs()
                .min(spare(&self.children[index], least[index]));
            self.children[index].weight -= given;
            self.children[into].weight += given;
            -(given as isize)
        }
    }
}

mod cells;
mod pair;
#[cfg(test)]
mod tests;
mod tiling;
