//! Where a row of a pane's text stands in its region: the column of its line at the region's
//! left edge, how far in its text starts, and between which lines of the pane's it is drawn.
//!
//! Must not depend on: what a row holds, or glyphs.

use oso_core::frame::{CellRect, PixelRect};
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
    /// indented (ADR 0165).
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

    /// The same cut to between lines `above` and `below` pixels thick along the region's edges,
    /// none where 0, with no band past one (ADR 0170).
    pub(crate) fn with_rules(self, above: f32, below: f32) -> Self {
        Self {
            clip: PixelRect {
                y: self.clip.y + above,
                height: (self.clip.height - above - below).max(0.0),
                ..self.clip
            },
            keeps_band_below: self.keeps_band_below && below == 0.0,
            ruled_above: above > 0.0,
            ..self
        }
    }
}
