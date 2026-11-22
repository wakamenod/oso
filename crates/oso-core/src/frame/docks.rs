//! Docked windows (ADR 0170): the rows a tiled window gives up at its bottom, above its
//! modeline, to the windows docked under its buffer, top to bottom.
//!
//! Must not depend on: `editor`; it lays out rectangles for window ids.
//!
//! - The host keeps at least [`HOST_LEAST`] rows of text, and each dock its least. Past
//!   those, the rows go to the docks by their ranks, the lowest first, each up to what it
//!   wants; the rest stay the host's.
//! - A host with too few rows for that hides its docks, which then get no cells, and says so.
//! - A docked window spans the host's gutter and text, its own gutter of the columns it asks
//!   for at its left; it has no modeline and no divider, and the host's divider runs beside it.

use super::{CellRect, FrameLayout, WindowRegions};
use crate::window::WindowId;

/// The fewest text rows a host keeps.
pub(crate) const HOST_LEAST: usize = 2;

/// A docked window's ask, in the order it is laid out, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DockAsk {
    pub(crate) window: WindowId,
    /// The rows it wants.
    pub(crate) rows: usize,
    /// The fewest rows it shows with.
    pub(crate) least: usize,
    /// Its place in the order the rows past the least go: lower first.
    pub(crate) rank: u8,
    /// Its gutter's columns.
    pub(crate) gutter: usize,
}

impl FrameLayout {
    /// Lays `asks` out, top to bottom, inside `host`'s text, from its bottom: each a window of
    /// this layout after `host`. Returns whether they have cells; `host` with too few rows hides
    /// them, and is listed in [`FrameLayout::docks_hidden`].
    pub(crate) fn dock(&mut self, host: WindowId, asks: &[DockAsk]) -> bool {
        let Some(at) = self.windows.iter().position(|(id, _)| *id == host) else {
            return false;
        };
        let regions = self.windows[at].1;
        let least: usize = asks.iter().map(|ask| ask.least).sum();
        let room = regions.text.rows.saturating_sub(HOST_LEAST);
        if asks.is_empty() || room < least || regions.text.columns == 0 {
            if !asks.is_empty() {
                self.docks_hidden.push(host);
            }
            return false;
        }
        let rows = rows_given(asks, room - least);
        let total: usize = rows.iter().sum();
        let host_rows = regions.text.rows - total;
        let area = regions.area();
        let mut row = regions.text.row + host_rows;
        let mut placed = Vec::with_capacity(asks.len());
        for (ask, &rows) in asks.iter().zip(&rows) {
            let gutter = ask.gutter.min(area.columns);
            let docked = WindowRegions {
                gutter: CellRect {
                    row,
                    column: area.column,
                    rows,
                    columns: gutter,
                },
                text: CellRect {
                    row,
                    column: area.column + gutter,
                    rows,
                    columns: area.columns - gutter,
                },
                modeline: CellRect {
                    row: row + rows,
                    rows: 0,
                    ..area
                },
                divider: None,
            };
            row += rows;
            if rows > 0 {
                self.docked_in.push((ask.window, host));
            }
            placed.push((
                ask.window,
                if rows > 0 {
                    docked
                } else {
                    WindowRegions::EMPTY
                },
            ));
        }
        let host_regions = &mut self.windows[at].1;
        host_regions.text.rows = host_rows;
        host_regions.gutter.rows = host_rows;
        self.windows.splice(at + 1..at + 1, placed);
        self.index();
        true
    }

    /// The window next to `window` on `side` if `window` is a host or a docked window with one
    /// there, as [`FrameLayout::neighbour`] has it: `Some(None)` where there is none. `None`
    /// where the tree's adjacency decides.
    pub(super) fn docked_neighbour(
        &self,
        window: WindowId,
        side: super::Side,
        cell: usize,
    ) -> Option<Option<WindowId>> {
        use super::Side;
        let docks_of = |host: WindowId| {
            (self.docked_in.iter())
                .filter(move |(_, of)| *of == host)
                .map(|(docked, _)| *docked)
        };
        let host =
            (self.docked_in.iter()).find_map(|(docked, host)| (*docked == window).then_some(*host));
        match (host, side) {
            // From a host, down goes to its first dock.
            (None, Side::Down) => docks_of(window).next().map(Some),
            (None, _) => None,
            (Some(host), Side::Up) => {
                let above = docks_of(host).take_while(|docked| *docked != window).last();
                Some(above.or(Some(host)))
            }
            (Some(host), Side::Down) => {
                let below = docks_of(host).skip_while(|docked| *docked != window).nth(1);
                Some(below.or_else(|| self.neighbour(host, Side::Down, cell)))
            }
            (Some(host), Side::Left | Side::Right) => Some(self.neighbour(host, side, cell)),
        }
    }

    /// The hosts whose docks this layout hides, as each has too few rows for them.
    pub(crate) fn docks_hidden(&self) -> &[WindowId] {
        &self.docks_hidden
    }
}

/// The rows each of `asks` gets: its least, then of `extra`, in the order of their ranks, each
/// up to what it wants.
fn rows_given(asks: &[DockAsk], mut extra: usize) -> Vec<usize> {
    let mut rows: Vec<usize> = asks
        .iter()
        .map(|ask| ask.least.min(ask.rows.max(ask.least)))
        .collect();
    let mut order: Vec<usize> = (0..asks.len()).collect();
    order.sort_by_key(|&at| asks[at].rank);
    for at in order {
        let more = asks[at].rows.saturating_sub(rows[at]).min(extra);
        rows[at] += more;
        extra -= more;
    }
    rows
}

#[cfg(test)]
mod tests;
