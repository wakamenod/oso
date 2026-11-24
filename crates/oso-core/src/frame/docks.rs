//! Docked windows (ADR 0170): the rows a tiled window gives up at its bottom, above its
//! modeline, to the windows docked under its buffer, top to bottom.
//!
//! Must not depend on: `editor`; it lays out rectangles for window ids.
//!
//! - The host keeps at least [`HOST_LEAST`] rows of text, and each dock its least. Past
//!   those, the rows go to the docks by their ranks, the lowest first, each up to what it
//!   wants; the rest stay the host's.
//! - A host with too few rows for that hides its docks, which then get no cells, and says so.
//! - A docked window may have a row above and below its text, which is its padding, and its host a
//!   footer, a row under its docks. They are given last, from what every ask leaves over: the
//!   padding both its rows or none, then the footer, so that a short host gives up the footer
//!   first, then the padding.
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
    /// Whether it wants a row above and below its text.
    pub(crate) padded: bool,
}

/// What a layout has beside its windows' regions, for its docks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct Around {
    /// The rows above and below the text of a docked window, which it has both or none of.
    padding: Vec<(WindowId, [CellRect; 2])>,
    /// Each host's footer.
    footers: Vec<(WindowId, CellRect)>,
}

impl Around {
    pub(super) fn clear(&mut self) {
        self.padding.clear();
        self.footers.clear();
    }
}

impl FrameLayout {
    /// Lays `asks` out, top to bottom, inside `host`'s text, from its bottom: each a window of
    /// this layout after `host`. Returns whether they have cells; `host` with too few rows hides
    /// them, and is listed in [`FrameLayout::docks_hidden`]. With `footer`, a row is under them, where
    /// there is room for it.
    pub(crate) fn dock(&mut self, host: WindowId, asks: &[DockAsk], footer: bool) -> bool {
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
        let mut total: usize = rows.iter().sum();
        // What is around them, of what they left over.
        let padded = |at: usize| asks[at].padded && rows[at] > 0;
        let wanted = 2 * (0..asks.len()).filter(|&at| padded(at)).count();
        let fits = total + wanted <= room;
        if fits {
            total += wanted;
        }
        let footer = footer && fits && total < room;
        total += usize::from(footer);
        let host_rows = regions.text.rows - total;
        let area = regions.area();
        let mut row = regions.text.row + host_rows;
        let mut placed = Vec::with_capacity(asks.len());
        for (at, (ask, &rows)) in asks.iter().zip(&rows).enumerate() {
            let padded = fits && padded(at);
            let above = CellRect {
                row,
                rows: 1,
                ..area
            };
            row += usize::from(padded);
            let docked = docked_regions(CellRect { row, rows, ..area }, ask.gutter);
            row += rows;
            if padded {
                let below = CellRect { row, ..above };
                self.around.padding.push((ask.window, [above, below]));
                row += 1;
            }
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
        if footer {
            let under = CellRect {
                row,
                rows: 1,
                ..area
            };
            self.around.footers.push((host, under));
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

    /// The row under what is docked in `host`, above its modeline, from its left edge to its right:
    /// none while it has no docks, its buffer no footer ([`Editor::set_footer`]), or the host too
    /// few rows for it.
    ///
    /// [`Editor::set_footer`]: crate::editor::Editor::set_footer
    pub fn footer(&self, host: WindowId) -> Option<CellRect> {
        let mut footers = self.around.footers.iter();
        footers.find_map(|(of, row)| (*of == host).then_some(*row))
    }

    /// The rows above and below the text of `docked`, a docked window, that are its own and hold
    /// none of it, from its host's left edge to its right: none for one that has none, or whose
    /// host has too few rows for them.
    pub fn padding(&self, docked: WindowId) -> Option<[CellRect; 2]> {
        let mut padding = self.around.padding.iter();
        padding.find_map(|(of, rows)| (*of == docked).then_some(*rows))
    }

    /// Which row around `window`'s docks, or its own text, the cell at `row` and `column` is in.
    pub(super) fn around_docks(
        &self,
        window: WindowId,
        row: usize,
        column: usize,
    ) -> Option<super::Part> {
        let holds = |rect: &CellRect| rect.contains(row, column);
        if self.footer(window).as_ref().is_some_and(holds) {
            return Some(super::Part::Footer);
        }
        let padding = self.padding(window);
        padding
            .is_some_and(|rows| rows.iter().any(holds))
            .then_some(super::Part::Padding)
    }

    /// The hosts whose docks this layout hides, as each has too few rows for them.
    pub(crate) fn docks_hidden(&self) -> &[WindowId] {
        &self.docks_hidden
    }
}

/// The regions of a docked window in `area`, its gutter of `gutter` columns at its left.
fn docked_regions(area: CellRect, gutter: usize) -> WindowRegions {
    let gutter = gutter.min(area.columns);
    WindowRegions {
        gutter: CellRect {
            columns: gutter,
            ..area
        },
        text: CellRect {
            column: area.column + gutter,
            columns: area.columns - gutter,
            ..area
        },
        modeline: CellRect {
            row: area.row + area.rows,
            rows: 0,
            ..area
        },
        divider: None,
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
