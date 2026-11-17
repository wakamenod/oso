//! The budget a review's sides' runs are held in (ADR 0122, step 9): what they cost, keeping
//! a side's within it, and the room files on screen take from those farthest from them.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system.
//!
//! - A side's runs cost their runs and their lines, as each line is kept too.
//! - Past the budget, a file on screen takes the room of the files off screen farthest from it
//!   in the review's order, whose documents close too. A side that still does not fit keeps
//!   no runs and is not opened again until other files show, so that a screen whose sides hold
//!   more than the budget is parsed once, not over and over.

use oso_core::review::Side;

use super::Highlighting;
use super::sides::index;
use crate::review::view::project::Projected;

/// The most a review's sides hold over its lines, in runs and lines: about what one 2 MiB
/// buffer can hold.
pub(crate) const MAX_RUNS: usize = 2_000_000;

/// What `projected` costs.
pub(super) fn cost(projected: &Projected) -> usize {
    projected.runs() + projected.lines()
}

impl Highlighting {
    /// Keeps `projected` as `path`'s `side`'s runs, within the budget.
    pub(super) fn keep(
        &mut self,
        path: &[u8],
        side: Side,
        projected: Projected,
        order: &[Box<[u8]>],
    ) {
        let before = self.files.get(path).and_then(|file| {
            let state = &file.sides[index(side)];
            state.projected.as_ref().map(cost)
        });
        let others = self.held.saturating_sub(before.unwrap_or(0));
        let wanted = others.saturating_add(cost(&projected));
        if wanted > self.budget && self.shown.contains_key(path) {
            self.make_room(wanted - self.budget, path, order);
        }
        let others = self.held.saturating_sub(before.unwrap_or(0));
        let fits = others.saturating_add(cost(&projected)) <= self.budget;
        let screen = self.screen;
        let Some(file) = self.files.get_mut(path) else {
            return;
        };
        let state = &mut file.sides[index(side)];
        if !fits {
            // Off screen, or more than the room a screen's files can make.
            if state.projected.take().is_some() {
                self.changed.insert(path.into());
            }
            self.held = others;
            state.complete = false;
            state.over = Some(screen);
            self.close(path, side);
            return;
        }
        self.held = others + cost(&projected);
        // Projected over every line the reading shows of the side once it is done.
        state.complete = projected.done;
        state.projected = Some(projected);
        self.changed.insert(path.into());
    }

    /// Drops the runs of files off screen, those farthest in `order` from `near` first, and
    /// closes their documents, until `needed` is free or nothing is left to drop.
    fn make_room(&mut self, mut needed: usize, near: &[u8], order: &[Box<[u8]>]) {
        let at = order.iter().position(|path| **path == *near).unwrap_or(0);
        let mut far: Vec<(usize, usize)> = (0..order.len())
            .filter(|index| !self.shown.contains_key(&order[*index]))
            .map(|index| (index.abs_diff(at), index))
            .collect();
        far.sort_unstable_by_key(|(distance, _)| std::cmp::Reverse(*distance));
        let screen = self.screen;
        for (_, index_in_order) in far {
            if needed == 0 {
                return;
            }
            let path = &order[index_in_order];
            for side in [Side::Old, Side::New] {
                let Some(file) = self.files.get_mut(path) else {
                    break;
                };
                let state = &mut file.sides[index(side)];
                let dropped = state.projected.take();
                let open = state.open.is_some();
                if dropped.is_none() && !open {
                    continue;
                }
                state.complete = false;
                state.over = Some(screen);
                if let Some(dropped) = dropped {
                    needed = needed.saturating_sub(cost(&dropped));
                    self.held = self.held.saturating_sub(cost(&dropped));
                    self.changed.insert(path.clone());
                }
                self.close(path, side);
            }
        }
    }
}
