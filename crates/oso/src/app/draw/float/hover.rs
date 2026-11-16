//! A server's description at point (ADR 0138, Hover, step 10): a float's panel right below the
//! cursor's row, or above it, and the description's buffer in it, highlighted and wrapped as
//! the editor lays it out, with no cursor.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::display_map::DisplayPoint;
use oso_core::editor::Editor;
use oso_core::window::Screen;
use oso_render::{
    Bands, CursorStyle, Layer, Pane, PaneContent, PaneId, Panel, TextMarks, TextView,
};

/// The base of the description's panes, past every other float's.
const HOVER_PANES: u32 = 1 << 22;

/// Adds the description's panes to `list`, if one shows: its panel over the tiled panes, and
/// its text as `screen` holds it.
pub(in crate::app::draw) fn hover_panes<'a>(
    editor: &'a Editor,
    screen: &'a Screen,
    list: &mut Vec<Pane<'a>>,
) {
    let (Some(regions), Some(shown)) = (editor.hover_layout(), editor.hover()) else {
        return;
    };
    let Some(buffer) = editor.buffer(shown.buffer()) else {
        return;
    };
    list.push(Pane {
        id: PaneId(HOVER_PANES),
        region: editor.frame().tiled(),
        layer: Layer::FLOAT,
        background: None,
        panel: Some(Panel {
            cells: regions.outer,
            field: None,
            field_room: regions.outer,
            keys: &[],
            selected: None,
            marked: None,
            // Framed, as a completion's float is, so it stands apart from the text beneath.
            border: true,
        }),
        bands: Bands::NONE,
        content: PaneContent::Lines(&[]),
    });
    let view = TextView {
        display: buffer.display(),
        buffer: shown.buffer(),
        revision: buffer.revision(),
        screen,
        cursor: DisplayPoint::new(0, 0),
        cursor_style: CursorStyle::None,
        others: &[],
        preedit: None,
        fills: &[],
        labels: &[],
        whitespace: false,
        flash: None,
        highlights: buffer.highlights(),
        marks: TextMarks::NONE,
    };
    list.push(Pane {
        id: PaneId(HOVER_PANES + 1),
        region: regions.inner,
        layer: Layer::FLOAT_CONTENT,
        background: None,
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Text(view),
    });
}
