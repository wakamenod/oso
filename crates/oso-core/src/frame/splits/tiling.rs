//! The window tree as data (ADR 0112): what a restart hands over and takes back, with each
//! child's weight, its windows in the tree's order.
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type; it turns a tree
//! into data and back.

use std::fmt;

use super::{Child, Node, Run, Splits, Way};
use crate::window::WindowId;

/// The most runs one inside another a tiling may hold, as a restart takes it (ADR 0112).
pub(crate) const MAX_DEPTH: usize = 16;

/// A frame's tiled windows as data: windows, and runs of two or more children laid out the same
/// way, each child with its weight. The windows are in the tree's order, left to right, top to
/// bottom, so a leaf names none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tiling {
    /// A window, the next in the tree's order.
    Window,
    /// Two or more children laid out `way`, each with its weight.
    Run {
        way: Way,
        children: Vec<(usize, Tiling)>,
    },
}

impl Tiling {
    /// How many windows it holds.
    pub fn windows(&self) -> usize {
        match self {
            Self::Window => 1,
            Self::Run { children, .. } => children.iter().map(|(_, child)| child.windows()).sum(),
        }
    }
}

/// Why a tiling was refused. Nothing changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadTiling {
    /// The frame has more than one window already, or a prompt is open.
    Split,
    /// A run has fewer than two children.
    LonelyRun,
    /// A child has a weight of 0.
    NoWeight,
    /// Runs nest deeper than [`MAX_DEPTH`].
    TooDeep,
    /// Weights too large to merge a run into another of the same way.
    TooHeavy,
}

impl fmt::Display for BadTiling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Split => "the frame is split already, or a prompt is open",
            Self::LonelyRun => "a run of windows has fewer than two",
            Self::NoWeight => "a window or run has no weight",
            Self::TooDeep => "runs of windows nest too deep",
            Self::TooHeavy => "the weights are too large",
        })
    }
}

impl std::error::Error for BadTiling {}

impl Splits {
    /// The tree as data, with each child's weight.
    pub(crate) fn tiling(&self) -> Tiling {
        self.root.tiling()
    }

    /// A tree laid out as `tiling` says, its first window `first` and the others named from
    /// `next` on, which this moves past them. Returns it and its windows in the tree's order.
    /// A run inside another of the same way is merged into it, as the tree keeps it, its
    /// weights scaled so that every window keeps its share.
    ///
    /// # Errors
    ///
    /// [`BadTiling`] for a run of fewer than two children, a weight of 0, runs deeper than
    /// [`MAX_DEPTH`], or weights too large to merge.
    pub(crate) fn from_tiling(
        tiling: &Tiling,
        first: WindowId,
        next: &mut u64,
    ) -> Result<(Self, Vec<WindowId>), BadTiling> {
        let mut ids = Vec::with_capacity(tiling.windows());
        let mut name = || {
            let id = if ids.is_empty() {
                first
            } else {
                let id = WindowId::new(*next);
                *next += 1;
                id
            };
            ids.push(id);
            id
        };
        let root = build(tiling, 0, &mut name)?;
        Ok((Self { root, gutter: 0 }, ids))
    }
}

impl Node {
    fn tiling(&self) -> Tiling {
        match self {
            Self::Window(_) => Tiling::Window,
            Self::Run(run) => Tiling::Run {
                way: run.way,
                children: run
                    .children
                    .iter()
                    .map(|child| (child.weight, child.node.tiling()))
                    .collect(),
            },
        }
    }
}

/// The node `tiling` describes, `depth` runs deep, its windows named by `name` in order.
fn build(
    tiling: &Tiling,
    depth: usize,
    name: &mut dyn FnMut() -> WindowId,
) -> Result<Node, BadTiling> {
    let (way, children) = match tiling {
        Tiling::Window => return Ok(Node::Window(name())),
        Tiling::Run { way, children } => (*way, children),
    };
    if depth >= MAX_DEPTH {
        return Err(BadTiling::TooDeep);
    }
    if children.len() < 2 {
        return Err(BadTiling::LonelyRun);
    }
    let mut built = Vec::with_capacity(children.len());
    for (weight, child) in children {
        if *weight == 0 {
            return Err(BadTiling::NoWeight);
        }
        built.push(Child {
            weight: *weight,
            node: build(child, depth + 1, name)?,
        });
    }
    Ok(Node::Run(Run {
        way,
        children: merged(way, built)?,
    }))
}

/// `children` of a run laid out `way`, with any child run of the same way merged into it:
/// each weight is scaled by the product of the merged runs' totals, so that every share stays
/// what it was in whole numbers.
fn merged(way: Way, children: Vec<Child>) -> Result<Vec<Child>, BadTiling> {
    let same = |child: &Child| matches!(&child.node, Node::Run(run) if run.way == way);
    if !children.iter().any(same) {
        return Ok(children);
    }
    let total = |run: &Run| run.children.iter().map(|child| child.weight).sum::<usize>();
    let mut product: usize = 1;
    for child in &children {
        if let Node::Run(run) = &child.node
            && run.way == way
        {
            product = product.checked_mul(total(run)).ok_or(BadTiling::TooHeavy)?;
        }
    }
    let mut flat = Vec::new();
    for child in children {
        match child.node {
            Node::Run(run) if run.way == way => {
                let scale = product / total(&run);
                for inner in run.children {
                    let weight = child
                        .weight
                        .checked_mul(inner.weight)
                        .and_then(|weight| weight.checked_mul(scale))
                        .ok_or(BadTiling::TooHeavy)?;
                    flat.push(Child { weight, ..inner });
                }
            }
            node => {
                let weight = child
                    .weight
                    .checked_mul(product)
                    .ok_or(BadTiling::TooHeavy)?;
                flat.push(Child { weight, node });
            }
        }
    }
    Ok(flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Tiling = Tiling::Window;

    fn run(way: Way, children: Vec<(usize, Tiling)>) -> Tiling {
        Tiling::Run { way, children }
    }

    fn from(tiling: &Tiling) -> Result<(Splits, Vec<WindowId>), BadTiling> {
        let mut next = 10;
        Splits::from_tiling(tiling, WindowId::new(0), &mut next)
    }

    #[test]
    fn a_tiling_names_its_windows_in_order_and_comes_back_as_it_went() {
        let tiling = run(
            Way::Across,
            vec![(3, W), (2, run(Way::Down, vec![(1, W), (1, W), (5, W)]))],
        );
        assert_eq!(tiling.windows(), 4);
        let (splits, ids) = from(&tiling).unwrap();
        let ids: Vec<u64> = ids.iter().map(|id| id.get()).collect();
        assert_eq!(ids, [0, 10, 11, 12], "the first window, then new names");
        assert_eq!(splits.iter().map(WindowId::get).collect::<Vec<_>>(), ids);
        assert_eq!(splits.tiling(), tiling);
    }

    #[test]
    fn a_run_inside_one_of_the_same_way_is_merged_and_every_share_kept() {
        // 1/3, then 2/3 split 1:3, so 1/3, 1/6 and 1/2: 4, 2 and 6 of 12.
        let tiling = run(
            Way::Across,
            vec![(1, W), (2, run(Way::Across, vec![(1, W), (3, W)]))],
        );
        let (splits, _) = from(&tiling).unwrap();
        assert_eq!(
            splits.tiling(),
            run(Way::Across, vec![(4, W), (2, W), (6, W)])
        );
    }

    #[test]
    fn a_tiling_the_tree_cannot_keep_is_refused() {
        let lonely = run(Way::Down, vec![(1, W)]);
        assert_eq!(from(&lonely).unwrap_err(), BadTiling::LonelyRun);
        let weightless = run(Way::Down, vec![(1, W), (0, W)]);
        assert_eq!(from(&weightless).unwrap_err(), BadTiling::NoWeight);
        let heavy = run(
            Way::Down,
            vec![(usize::MAX, W), (1, run(Way::Down, vec![(2, W), (2, W)]))],
        );
        assert_eq!(from(&heavy).unwrap_err(), BadTiling::TooHeavy);
        // Runs that alternate their way nest as deep as they are written.
        let nest = |depth: usize| {
            (0..depth).fold(W, |inner, level| {
                let way = if level % 2 == 0 {
                    Way::Down
                } else {
                    Way::Across
                };
                run(way, vec![(1, W), (1, inner)])
            })
        };
        assert!(from(&nest(MAX_DEPTH)).is_ok());
        assert_eq!(from(&nest(MAX_DEPTH + 1)).unwrap_err(), BadTiling::TooDeep);
    }
}
