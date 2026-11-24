//! The window that carries what is docked under a native session's transcript (ADR 0170).
//!
//! Must not depend on: what is docked, or who asks; it reads the frame's layout.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::frame::WindowRegions;
use oso_core::window::{DockSlot, WindowId};

/// The window that carries what is docked under `transcript`, as the dock in `slot` is laid out: the
/// one of the frame that shows it and whose own rows hold the dock's. None while no row of the dock
/// shows.
pub(super) fn carrier(
    editor: &Editor,
    transcript: BufferId,
    slot: DockSlot,
) -> Option<(WindowId, &WindowRegions)> {
    let dock = editor.dock_window(transcript, slot)?;
    let inside = editor.regions(dock)?.text;
    if inside.rows == 0 || inside.columns == 0 {
        return None;
    }
    (editor.frame().windows().iter()).find_map(|(id, regions)| {
        let window = editor.window_by_id(*id)?;
        let holds = (regions.text.row..regions.modeline.row).contains(&inside.row)
            && (regions.gutter.column..regions.text.column + regions.text.columns)
                .contains(&inside.column);
        let its = window.docked().is_none() && window.buffer() == transcript && holds;
        (its && regions.text.columns > 0).then_some((*id, regions))
    })
}
