//! The preview beside a float's list (ADR 0088, ADR 0091, ADR 0114): the column that divides
//! them, and the selected buffer, a kill's lines, the lines read around a project search's row,
//! or what is said in place of what may hold secrets.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::buffer::BufferId;
use oso_core::display_map::{DisplayPoint, DisplaySnapshot};
use oso_core::editor::{Editor, Preview};
use oso_core::frame::CellRect;
use oso_core::syntax::Highlights;
use oso_core::window::Screen;
use oso_render::{Bands, CursorStyle, Face, Layer, Pane, PaneContent, PaneId, TextMarks, TextView};

use super::{Lines, Parts, Screens};
use crate::app::draw::transcript::styled;

/// The base of a preview pane's id: each previewed buffer's pane has its own, so that the rows
/// shaped for it are kept while that buffer stays selected (ADR 0088).
const PREVIEW_PANES: u32 = 1 << 20;

/// The pane of the lines read around a project search's row, each read at a revision of its own
/// (ADR 0114).
const EXCERPT_PANE: u32 = 13;

/// The pane of what is said of a recent file no buffer visits (ADR 0148).
const NOT_OPEN_PANE: u32 = 14;

/// Adds a preview's panes to `list`: the column that divides it from the candidates, and the
/// selected buffer from where its window was, read-only with its cursor hollow, or what is
/// said of a buffer that may hold secrets in its place (ADR 0088), with a project search's
/// matches filled behind its text (ADR 0114).
pub(super) fn preview_panes<'a>(
    editor: &'a Editor,
    (parts, screens): (&'a Parts<'a>, Screens<'a>),
    lines: &'a Lines<'a>,
    list: &mut Vec<Pane<'a>>,
) {
    let regions = parts.layout.regions;
    let preview = regions.preview;
    let screen = screens.preview;
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
        Some(Preview::NotOpen) => pane(NOT_OPEN_PANE, preview, PaneContent::Lines(&lines.not_open)),
        Some(Preview::Text { .. }) => pane(12, preview, PaneContent::Lines(&lines.kill)),
        Some(Preview::Excerpt { revision, cursor }) => {
            let (Some(display), Some(prompt)) = (editor.preview_excerpt_display(), editor.prompt())
            else {
                return;
            };
            let cursor = display.offset_to_display_point(cursor);
            // Its rows are shaped in a pane of their own, so the prompt's id only names them.
            // An excerpt is not parsed, so it shows uncoloured.
            let shown = (prompt.buffer(), revision, cursor);
            let view = hollow(display, screen, shown, (screens.preview_fills, None), false);
            pane(EXCERPT_PANE, preview, PaneContent::Text(view))
        }
        Some(Preview::Buffer { id, cursor, .. }) => {
            let Some(buffer) = editor.buffer(id) else {
                return;
            };
            let display = buffer.display();
            let cursor = display.offset_to_display_point(cursor);
            // From the row that holds the start, wrapped at the preview's width while the
            // buffer wraps (ADR 0100).
            let shown = (id, buffer.revision(), cursor);
            let fills = screens.preview_fills;
            let look = (fills, buffer.highlights());
            let view = hollow(display, screen, shown, look, styled(buffer));
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

/// A preview's view of a buffer's `display` as `screen` shows it: the buffer, its revision and
/// its cursor in `shown`, hollow and read-only, with `fills` behind its text, `highlights` and
/// whether they give it styles.
/// Floats have no visual aids (ADR 0097), and a preview no diagnostics.
fn hollow<'a>(
    display: DisplaySnapshot,
    screen: &'a Screen,
    (buffer, revision, cursor): (BufferId, u64, DisplayPoint),
    (fills, highlights): (&'a [(Face, &'a [CellRect])], Option<&'a Highlights>),
    styled: bool,
) -> TextView<'a> {
    TextView {
        display,
        buffer,
        revision,
        screen,
        cursor,
        cursor_style: CursorStyle::Hollow,
        others: &[],
        preedit: None,
        fills,
        labels: &[],
        whitespace: false,
        flash: None,
        highlights,
        styled,
        marks: TextMarks::NONE,
    }
}
