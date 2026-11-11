//! The tree of a frame's tiled windows, and how it divides the frame's cells among them
//! (ADR 0112).
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type. The tree holds
//! window ids only; the editor keeps the windows.
//!
//! A leaf is a window. A run is two or more children laid out the same way, side by side or
//! stacked, and never has a child that is a run of the same way. Each child keeps a weight,
//! and a layout turns the weights into whole cells by the largest remainder, so a frame that
//! is resized keeps the proportions, and resizing it back gives the same cells. A change of
//! the tree first sets the weights it touches to the cells laid out, so that it works in
//! cells and keeps what the person sees.

use super::{CellRect, FrameLayout, FrameSize, WindowRegions, fitting_gutter};
use crate::window::WindowId;

/// The fewest text rows each half of a split keeps, above its modeline.
const SPLIT_ROWS: usize = 2;
/// The fewest text columns each half of a split keeps, beside its gutter: the narrowest a
/// window wraps at (ADR 0104).
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

/// The tiled windows of a frame, as a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splits {
    root: Node,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Child {
    weight: usize,
    node: Node,
}

impl Splits {
    /// A frame of one window.
    pub fn new(window: WindowId) -> Self {
        Self {
            root: Node::Window(window),
        }
    }

    /// The windows in the tree's order: left to right, top to bottom.
    pub fn iter(&self) -> impl Iterator<Item = WindowId> + '_ {
        let mut windows = Vec::new();
        self.root.windows(&mut windows);
        windows.into_iter()
    }

    /// Whether `window` is in the tree.
    pub fn contains(&self, window: WindowId) -> bool {
        self.root.contains(window)
    }

    /// Splits `at` into two, `at` above or at the left and `new` below or at the right, as
    /// `layout` lays it out. They share its cells, `new` taking the smaller half when they
    /// are odd, and no other window changes. Refused when either half would keep fewer than
    /// 2 text rows above its modeline, or 10 text columns beside its gutter.
    pub fn split(
        &mut self,
        at: WindowId,
        way: Way,
        new: WindowId,
        layout: &FrameLayout,
    ) -> Result<(), TooSmall> {
        let regions = layout.regions(at).ok_or(TooSmall)?;
        let outer = regions.outer();
        let (total, first_least, second_least) = match way {
            Way::Down => (outer.rows, SPLIT_ROWS + 1, SPLIT_ROWS + 1),
            Way::Across => {
                let least = regions.gutter.columns + SPLIT_COLUMNS;
                let divider = usize::from(regions.divider.is_some());
                (outer.columns, least + 1, least + divider)
            }
        };
        let second = total / 2;
        let first = total - second;
        if first < first_least || second < second_least || self.contains(new) {
            return Err(TooSmall);
        }
        self.root.split(at, way, new, [first, second], layout);
        Ok(())
    }

    /// Takes `window` out of the tree, its cells going to the window or run before it, or
    /// after it when it is the first. Returns whether it did: the only window stays.
    pub fn remove(&mut self, window: WindowId, layout: &FrameLayout) -> bool {
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
    pub fn balance(&mut self) {
        self.root.balance();
    }

    /// Makes `window` `cells` larger along `way`, or smaller when negative, as `layout` lays
    /// it out. It grows into the windows after it in the nearest run of that way, then those
    /// before it, and shrinks into the one after it, or before it when it is the last. None
    /// goes below the smallest a split leaves. Returns how many cells it grew, negative for
    /// how many it shrank.
    pub fn resize(
        &mut self,
        window: WindowId,
        way: Way,
        cells: isize,
        layout: &FrameLayout,
    ) -> isize {
        self.root.resize(window, way, cells, layout).unwrap_or(0)
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
        match self {
            Self::Window(id) if *id == at => {
                *self = Self::Run(Run {
                    way,
                    children: vec![
                        Child {
                            weight: first,
                            node: Self::Window(at),
                        },
                        Child {
                            weight: second,
                            node: Self::Window(new),
                        },
                    ],
                });
                true
            }
            Self::Window(_) => false,
            Self::Run(run) => {
                let Some(index) = run.children.iter().position(|c| c.node.contains(at)) else {
                    return false;
                };
                if run.way == way && run.children[index].node == Self::Window(at) {
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
        layout: &FrameLayout,
    ) -> Option<isize> {
        let Self::Run(run) = self else {
            return None;
        };
        let index = run.children.iter().position(|c| c.node.contains(window))?;
        if let Some(moved) = run.children[index].node.resize(window, way, cells, layout) {
            return Some(moved);
        }
        (run.way == way).then(|| run.resize(index, cells, layout))
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

    /// The fewest cells it can take along `way` while each window keeps what a split leaves.
    fn least(&self, way: Way, layout: &FrameLayout) -> usize {
        match self {
            Self::Window(id) => {
                let Some(regions) = layout.regions(*id) else {
                    return 0;
                };
                match way {
                    Way::Down => SPLIT_ROWS + 1,
                    Way::Across => {
                        regions.gutter.columns
                            + SPLIT_COLUMNS
                            + usize::from(regions.divider.is_some())
                    }
                }
            }
            Self::Run(run) => {
                let each = run.children.iter().map(|c| c.node.least(way, layout));
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
                let rights = |i: usize| right || (run.way == Way::Across && i < last);
                let total = match run.way {
                    Way::Across => area.columns,
                    Way::Down => area.rows,
                };
                let weights: Vec<usize> = run.children.iter().map(|c| c.weight).collect();
                let needs: Vec<usize> = run
                    .children
                    .iter()
                    .enumerate()
                    .map(|(i, c)| c.node.needs(run.way, rights(i)))
                    .collect();
                let kept = run.children.iter().position(|c| c.node.contains(keep));
                let cells = distribute(total, &weights, &needs, kept);
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
        if self.children[index].node == Node::Window(window) {
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
                Node::Run(mut inner) if inner.way == self.way => {
                    inner.reset(layout);
                    self.children.splice(index..=index, inner.children);
                }
                node => self.children[index].node = node,
            }
        }
        removed
    }

    fn resize(&mut self, index: usize, cells: isize, layout: &FrameLayout) -> isize {
        self.reset(layout);
        let least: Vec<usize> = self
            .children
            .iter()
            .map(|c| c.node.least(self.way, layout))
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

/// A window's regions in `area`: a divider at its right while a window is to its right, its
/// modeline at its bottom, and its gutter at the left of the text while the text keeps a
/// column. A window that has all the tiled area lays out as the one window always has
/// (ADR 0057).
fn window_regions(area: CellRect, right: bool, gutter: usize) -> WindowRegions {
    let divider = (right && area.columns >= 2).then(|| CellRect {
        column: area.column + area.columns - 1,
        columns: 1,
        ..area
    });
    let columns = area.columns - usize::from(divider.is_some());
    let modeline_rows = area.rows.saturating_sub(1).min(1);
    let text_rows = area.rows - modeline_rows;
    let gutter = fitting_gutter(
        FrameSize {
            rows: area.rows,
            columns,
        },
        gutter,
    );
    let text = CellRect {
        rows: text_rows,
        ..area
    };
    WindowRegions {
        gutter: CellRect {
            columns: gutter,
            ..text
        },
        text: CellRect {
            column: area.column + gutter,
            columns: columns - gutter,
            ..text
        },
        modeline: CellRect {
            row: area.row + text_rows,
            rows: modeline_rows,
            columns,
            ..area
        },
        divider,
    }
}

/// Divides `total` cells among children of `weights` by the largest remainder, each getting
/// at least what it `needs`. When their needs do not fit, the last children get none, but
/// `kept`.
fn distribute(total: usize, weights: &[usize], needs: &[usize], kept: Option<usize>) -> Vec<usize> {
    let mut shown = vec![true; weights.len()];
    let mut needed: usize = needs.iter().sum();
    for i in (0..weights.len()).rev() {
        if needed <= total {
            break;
        }
        if Some(i) != kept {
            shown[i] = false;
            needed -= needs[i];
        }
    }
    let mut cells = vec![0; weights.len()];
    // Children held at their needs, which their share would not give them.
    let mut held = vec![false; weights.len()];
    loop {
        let free: Vec<usize> = (0..weights.len())
            .filter(|&i| shown[i] && !held[i])
            .collect();
        let fixed: usize = (0..weights.len())
            .filter(|&i| shown[i] && held[i])
            .map(|i| needs[i])
            .sum();
        let left = total.saturating_sub(fixed);
        let shares = largest_remainder(left, free.iter().map(|&i| weights[i]));
        let mut changed = false;
        for (&i, share) in free.iter().zip(shares) {
            cells[i] = share;
            if share < needs[i] && needed <= total {
                held[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for i in 0..weights.len() {
        if shown[i] && held[i] {
            cells[i] = needs[i];
        }
    }
    // Needs that do not fit even alone: the kept child takes what there is.
    let sum: usize = cells.iter().sum();
    if sum > total
        && let Some(i) = kept
    {
        cells.iter_mut().for_each(|c| *c = 0);
        cells[i] = total;
    }
    cells
}

/// `total` divided in proportion to `weights`, the cells left over by rounding down going
/// one each to the largest remainders, the earlier first on a tie.
fn largest_remainder(total: usize, weights: impl Iterator<Item = usize> + Clone) -> Vec<usize> {
    let sum: u128 = weights.clone().map(|w| w as u128).sum();
    if sum == 0 {
        return weights.map(|_| 0).collect();
    }
    let total_wide = total as u128;
    let mut shares: Vec<(usize, u128)> = weights
        .map(|w| {
            let exact = total_wide * w as u128;
            ((exact / sum) as usize, exact % sum)
        })
        .collect();
    let given: usize = shares.iter().map(|(cells, _)| cells).sum();
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by(|&a, &b| shares[b].1.cmp(&shares[a].1).then(a.cmp(&b)));
    for &i in order.iter().take(total - given) {
        shares[i].0 += 1;
    }
    shares.into_iter().map(|(cells, _)| cells).collect()
}

#[cfg(test)]
mod tests;
