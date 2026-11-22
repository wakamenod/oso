//! A window aside an area of the tree (ADR 0122, step 6b): a review's files pane, split at the
//! left of a window or of a whole pair, a given number of columns wide, and given back to the
//! area at its right when it goes; and a pull request's commit list (ADR 0167), split above a
//! window or a whole pair, a given number of rows tall, and given back to the area below it.
//!
//! Must not depend on: `editor`, `command`, or any GPU or window-system type.

use super::{FrameLayout, SPLIT_COLUMNS, SPLIT_ROWS, Splits, TooSmall, Way};
use crate::window::WindowId;

impl Splits {
    /// Splits the window `at`, or the pair it is in, `way`, `new` before it, at its left
    /// across or above it down, taking `cells` of its columns or rows, a divider or a modeline
    /// included, or fewer as the rest keeps its smallest. Refused, as [`Splits::split`]
    /// refuses, when either would keep less than its smallest.
    pub(crate) fn split_aside(
        &mut self,
        at: WindowId,
        new: WindowId,
        way: Way,
        cells: usize,
        layout: &FrameLayout,
    ) -> Result<(), TooSmall> {
        let unit = self.root.unit(at).ok_or(TooSmall)?;
        if self.contains(new) {
            return Err(TooSmall);
        }
        let other = match way {
            Way::Across => Way::Down,
            Way::Down => Way::Across,
        };
        let total = unit.extent(way, layout);
        // Across, the new window always has a divider at its right; the area keeps its own.
        let new_least = match way {
            Way::Across => self.gutter + SPLIT_COLUMNS + 1,
            Way::Down => SPLIT_ROWS + 1,
        };
        let rest_least = unit.least(way, layout, self.gutter);
        let first = cells.min(total.saturating_sub(rest_least));
        let other_ok = unit.extent(other, layout) >= unit.least(other, layout, self.gutter);
        if first < new_least || !other_ok {
            return Err(TooSmall);
        }
        self.root
            .split(at, way, new, [first, total - first], true, layout);
        Ok(())
    }

    /// Takes `window` out of the tree as [`Splits::remove`] does, its cells going to the window
    /// or run after it, as a window split at the left or above gives them back.
    pub(crate) fn remove_toward_next(&mut self, window: WindowId, layout: &FrameLayout) -> bool {
        self.remove_giving(window, layout, true)
    }
}
