//! A window aside an area of the tree (ADR 0122, step 6b): a review's files pane, split at the
//! left of a window or of a whole pair, a given number of columns wide, and given back to the
//! area at its right when it goes.
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.

use super::{Child, FrameLayout, Node, Run, SPLIT_COLUMNS, Splits, TooSmall, Way};
use crate::window::WindowId;

impl Splits {
    /// Splits the window `at`, or the pair it is in, across, `new` at its left taking `columns`
    /// of its cells, divider included, or fewer as the rest keeps its smallest. Refused, as
    /// [`Splits::split`] refuses, when either would keep less than its smallest.
    pub(crate) fn split_left(
        &mut self,
        at: WindowId,
        new: WindowId,
        columns: usize,
        layout: &FrameLayout,
    ) -> Result<(), TooSmall> {
        let unit = self.root.unit(at).ok_or(TooSmall)?;
        if self.contains(new) {
            return Err(TooSmall);
        }
        let total = unit.extent(Way::Across, layout);
        // The new window always has a divider at its right; the area keeps its own.
        let new_least = self.gutter + SPLIT_COLUMNS + 1;
        let rest_least = unit.least(Way::Across, layout, self.gutter);
        let first = columns.min(total.saturating_sub(rest_least));
        let across_ok =
            unit.extent(Way::Down, layout) >= unit.least(Way::Down, layout, self.gutter);
        if first < new_least || !across_ok {
            return Err(TooSmall);
        }
        self.root
            .split_before(at, new, [first, total - first], layout);
        Ok(())
    }

    /// Takes `window` out of the tree as [`Splits::remove`] does, its cells going to the window
    /// or run after it, as a window split at the left gives them back.
    pub(crate) fn remove_toward_next(&mut self, window: WindowId, layout: &FrameLayout) -> bool {
        self.remove_giving(window, layout, true)
    }
}

impl Node {
    /// Puts `new` before the unit of `at` across, `new` taking `first` cells and the unit
    /// `rest`.
    fn split_before(
        &mut self,
        at: WindowId,
        new: WindowId,
        [first, rest]: [usize; 2],
        layout: &FrameLayout,
    ) -> bool {
        if self.is_unit_of(at) {
            let unit = std::mem::replace(self, Self::Window(new));
            *self = Self::Run(Run {
                way: Way::Across,
                children: vec![
                    Child {
                        weight: first,
                        node: Self::Window(new),
                    },
                    Child {
                        weight: rest,
                        node: unit,
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
                if run.way == Way::Across && !run.pair && run.children[index].node.is_unit_of(at) {
                    run.reset(layout);
                    run.children[index].weight = rest;
                    run.children.insert(
                        index,
                        Child {
                            weight: first,
                            node: Self::Window(new),
                        },
                    );
                    return true;
                }
                run.children[index]
                    .node
                    .split_before(at, new, [first, rest], layout)
            }
        }
    }
}
