//! The preview beside a float's list (ADR 0088, ADR 0091): the column that divides them, and
//! the selected buffer, a kill's lines, or what is said in place of what may hold secrets.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::editor::{Editor, Preview};
use oso_core::frame::CellRect;
use oso_core::window::ViewTop;
use oso_render::{Bands, CursorStyle, Layer, Pane, PaneContent, PaneId, TextView};

use super::{Lines, Parts};

/// The base of a preview pane's id: each previewed buffer's pane has its own, so that the rows
/// shaped for it are kept while that buffer stays selected (ADR 0088).
const PREVIEW_PANES: u32 = 1 << 20;

/// Adds a preview's panes to `list`: the column that divides it from the candidates, and the
/// selected buffer from where its window was, read-only with its cursor hollow, or what is
/// said of a buffer that may hold secrets in its place (ADR 0088).
pub(super) fn preview_panes<'a>(
    editor: &'a Editor,
    parts: &'a Parts<'a>,
    lines: &'a Lines<'a>,
    list: &mut Vec<Pane<'a>>,
) {
    let regions = parts.layout.regions;
    let preview = regions.preview;
    if parts.list.is_none() || preview.rows == 0 || preview.columns == 0 {
        return;
    }
    let pane = |id, region, content| Pane {
        id: PaneId(id),
        region,
        layer: Layer::FLOAT_CONTENT,
        background: None,
        panel: None,
        bands: Bands::NONE,
        content,
    };
    let divider = CellRect {
        column: preview.column - 1,
        columns: 1,
        ..preview
    };
    let rows = &lines.divider[..preview.rows.min(lines.divider.len())];
    list.push(pane(10, divider, PaneContent::Lines(rows)));
    let content = match editor.preview() {
        None => return,
        Some(Preview::Withheld) => pane(11, preview, PaneContent::Lines(&lines.withheld)),
        Some(Preview::Text { .. }) => pane(12, preview, PaneContent::Lines(&lines.kill)),
        Some(Preview::Buffer { id, start, cursor }) => {
            let Some(buffer) = editor.buffer(id) else {
                return;
            };
            let display = buffer.display();
            let cursor = display.offset_to_display_point(cursor);
            // Logical lines, from the one that holds the start, until the renderer draws view
            // rows (ADR 0100).
            let top = display.offset_to_display_point(start).row;
            let view = TextView {
                display,
                buffer: id,
                revision: buffer.revision(),
                scroll: ViewTop { row: top, left: 0 },
                cursor,
                cursor_style: CursorStyle::Hollow,
                others: &[],
                preedit: None,
                fills: &[],
            };
            let number = u32::try_from(id.get()).unwrap_or(u32::MAX);
            pane(
                PREVIEW_PANES.saturating_add(number),
                preview,
                PaneContent::Text(view),
            )
        }
    };
    list.push(content);
}
