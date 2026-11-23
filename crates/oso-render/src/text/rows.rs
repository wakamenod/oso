//! Where a row of a pane's text stands in its region: the column of its line at the region's
//! left edge, and how far in its text starts.
//!
//! Must not depend on: what a row holds, or glyphs.

use oso_core::frame::CellRect;
use oso_core::window::ViewTop;

use super::Layout;

impl Layout {
    /// The same with `left`, a column of a row's line, at the region's left edge: a view
    /// row's start, or the window's left column on a line that does not wrap.
    pub(crate) fn with_left(self, left: usize) -> Self {
        Self {
            scroll: ViewTop {
                left,
                ..self.scroll
            },
            ..self
        }
    }

    /// The same for a row whose text starts `cells` into the region, as one its line goes on in
    /// indented (ADR 0165): nothing of it draws before there.
    pub(crate) fn indented(self, cells: usize) -> Self {
        let cells = cells.min(self.region.columns);
        Self {
            region: CellRect {
                column: self.region.column + cells,
                columns: self.region.columns - cells,
                ..self.region
            },
            ..self
        }
    }
}
