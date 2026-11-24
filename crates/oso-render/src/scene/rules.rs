//! The lines along a pane's edges (ADR 0170): where they are, and where the pane's text may be
//! drawn beside them.
//!
//! Must not depend on: `capture`, `paint` or a font backend.
//!
//! - A line is a rectangle in the pane's own cells, so it is cut to the pane as all it draws is
//!   (ADR 0061), and a frame costs two a ruled pane at most.

use oso_core::frame::PixelRect;
use oso_platform::text::CellMetrics;

use super::{Draws, line_width};
use crate::pane::{Pane, Rules};
use crate::text::Layout;

/// How much of a pane each of its `rules` takes, above and below, in whole pixels.
fn taken(rules: Rules, metrics: CellMetrics) -> [f32; 2] {
    let line = line_width(metrics);
    [rules.above, rules.below].map(|face| if face.is_some() { line } else { 0.0 })
}

/// `layout` with `rules`' lines out of where the pane's text goes, its bands past them too.
pub(super) fn between(layout: Layout, rules: Rules, metrics: CellMetrics) -> Layout {
    if rules == Rules::NONE {
        return layout;
    }
    let [above, below] = taken(rules, metrics);
    layout.with_rules(above, below)
}

/// Draws `pane`'s lines in `layout`, its unscrolled one.
pub(super) fn draw_rules(
    draws: &mut Draws,
    pane: &Pane<'_>,
    layout: &Layout,
    metrics: CellMetrics,
) {
    let Rules { above, below } = pane.rules;
    if pane.rules == Rules::NONE {
        return;
    }
    let Some(rect) = layout.frame_cells(pane.region) else {
        return;
    };
    let height = line_width(metrics).min(rect.height);
    let lines = [(above, rect.y), (below, rect.y + rect.height - height)];
    for (face, y) in lines {
        if let Some(face) = face {
            let ink = draws.theme.foreground(face);
            draws.fill_color(Some(PixelRect { y, height, ..rect }), ink);
        }
    }
}

#[cfg(test)]
mod tests;
