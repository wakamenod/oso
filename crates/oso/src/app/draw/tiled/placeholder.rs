//! An empty composer's placeholder (ADR 0170, as amended): what the composer is for and its keys,
//! dim, over its first row from the cell after the cursor's. It is drawn over the window and is
//! no text of the buffer's, so nothing that reads the buffer meets it.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::editor::Editor;
use oso_core::frame::{CellRect, WindowRegions};
use oso_core::minibuffer::cells_of;
use oso_core::window::{DockSlot, Screen, WindowId};
use oso_render::{Bands, Face, Layer, Pane, PaneContent, Span};

use super::{Part, pane_id};
use crate::app::sessions::empty_composer;

/// Just over the tiled panes, as it goes over a composer's text pane, and under every float.
const OVER_TILED: Layer = Layer(Layer::TILED.0 + 1);

/// The rows a placeholder may show as, the whole of it first: each of `texts` in one span.
pub(in crate::app::draw) fn rows(texts: &[String; 3]) -> [Span<'_>; 3] {
    // Dim on the float's background, which a composer has.
    texts.each_ref().map(|text| Span {
        text,
        face: Face::Prompt,
    })
}

/// The pane of docked window `id`'s placeholder over the first row of its `regions`, the first of
/// `rows` that fits, else the last cut; or none: with no rows, in no composer's window, or over text, text
/// being composed included.
pub(super) fn pane<'a>(
    editor: &Editor,
    (id, regions): (WindowId, &WindowRegions),
    screen: &Screen,
    rows: &'a [&'a [Span<'a>]],
) -> Option<Pane<'a>> {
    let window = editor.window_by_id(id)?;
    let composer = matches!(window.docked(), Some((_, DockSlot::Composer)));
    if !composer || screen.preedit().is_some() || !empty_composer(editor.shown(window)) {
        return None;
    }
    let text = regions.text;
    // The cursor, filled or hollow, is in the first cell.
    let region = CellRect {
        column: text.column + 1,
        rows: 1,
        columns: text.columns.saturating_sub(1),
        ..text
    };
    if text.rows == 0 || region.columns == 0 {
        return None;
    }
    let fits = |row: &&[Span<'_>]| {
        let cells: usize = row.iter().map(|span| cells_of(span.text)).sum();
        cells <= region.columns
    };
    let at = rows.iter().position(fits).or(rows.len().checked_sub(1))?;
    Some(Pane {
        id: pane_id(id, Part::Placeholder),
        region,
        layer: OVER_TILED,
        // The current line's band and the flash show behind it, as behind text.
        background: None,
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Lines(&rows[at..=at]),
    })
}
