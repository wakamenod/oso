//! The ids of a tiled window's panes, which keep each window's shaped rows from frame to frame.
//!
//! Must not depend on: what a pane shows.

use oso_core::window::WindowId;
use oso_render::PaneId;

/// The first pane id of the windows': the echo area's and the floats' are below it.
const FIRST: u32 = 16;
/// The panes a window may draw: those of [`Part`].
const PER_WINDOW: u32 = 10;

/// Which of a window's panes.
#[derive(Clone, Copy)]
pub(in crate::app::draw) enum Part {
    Text = 0,
    Gutter = 1,
    Modeline = 2,
    Divider = 3,
    Signs = 4,
    Placeholder = 5,
    /// A composer's rules, above and below its text (ADR 0170).
    RuleAbove = 6,
    RuleBelow = 7,
    /// A session's footer, and the model at its right edge.
    Footer = 8,
    Model = 9,
}

/// The id of a window's pane, from the window's id, so that a window keeps its shaped rows
/// when another is split or deleted (ADR 0112). Ids come round again only after 400 million
/// windows have been made, as `WindowId`s are never reused; two shown together would have to
/// be that far apart to share one, which no session reaches.
pub(in crate::app::draw) fn pane_id(window: WindowId, part: Part) -> PaneId {
    let slots = (u32::MAX - FIRST) / PER_WINDOW;
    let slot = u32::try_from(window.get() % u64::from(slots)).unwrap_or(0);
    PaneId(FIRST + slot * PER_WINDOW + part as u32)
}
