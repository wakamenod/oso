//! The panes of the completion popup at point (ADR 0138, Completion in the buffer, step 9): its
//! panel under or over the word's row, and its rows, each the item's label with what matched,
//! then its kind and detail as the annotation column, as a completion's float draws its rows.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use std::ops::Range;

use oso_core::editor::Editor;
use oso_core::frame::FloatRegions;
use oso_core::minibuffer;
use oso_core::window::Screen;
use oso_render::{Bands, Layer, Pane, PaneContent, PaneId, Panel, Span};

use super::row::candidate_spans;

/// The popup's panes' ids, apart from the windows', the floats' and the menus' (`1 << 21`),
/// as the menus' are: its panel's and its rows'.
const POPUP_PANES: u32 = 1 << 22;
const PANEL: PaneId = PaneId(POPUP_PANES);
const ROWS: PaneId = PaneId(POPUP_PANES + 1);

/// The popup's rows' spans, built once a frame, and where it goes.
pub(in crate::app::draw) struct PopupSpans<'a> {
    regions: FloatRegions,
    selected: Option<usize>,
    rows: Vec<Vec<Span<'a>>>,
}

impl<'a> PopupSpans<'a> {
    /// The open popup's rows, placed over the selected window as `screen` shows it, the frame's
    /// own rows of that window, if it has room.
    pub(in crate::app::draw) fn of(editor: &'a Editor, screen: &Screen) -> Option<Self> {
        let popup = editor.completion_popup()?;
        let regions = editor.completion_popup_regions(screen)?;
        let text = regions.inner;
        let named = popup.rows().map(|(_, row)| (row.text(), row.binding()));
        let column = minibuffer::annotation_column(named, text.columns);
        let mut selected = None;
        let mut highlights: Vec<Range<usize>> = Vec::new();
        let rows = (popup.rows().take(text.rows).enumerate())
            .map(|(index, (chosen, row))| {
                if chosen {
                    selected = Some(index);
                }
                popup.highlights(row, &mut highlights);
                candidate_spans(row, (0, &highlights), (text.columns, column), false)
            })
            .collect();
        Some(Self {
            regions,
            selected,
            rows,
        })
    }

    /// The rows as the pane takes them.
    pub(in crate::app::draw) fn lines(&'a self) -> Vec<&'a [Span<'a>]> {
        self.rows.iter().map(|row| &row[..]).collect()
    }

    /// Adds the popup's panes to `list`: its panel, framed as a completion's float, and its rows.
    pub(in crate::app::draw) fn panes(
        &self,
        editor: &'a Editor,
        lines: &'a [&'a [Span<'a>]],
        list: &mut Vec<Pane<'a>>,
    ) {
        let (outer, inner) = (self.regions.outer, self.regions.inner);
        let selected = self.selected.map(|index| oso_core::frame::CellRect {
            row: inner.row + index,
            rows: 1,
            ..inner
        });
        list.push(Pane {
            id: PANEL,
            region: editor.frame().tiled(),
            layer: Layer::FLOAT,
            background: None,
            panel: Some(Panel {
                cells: outer,
                field: None,
                field_room: outer,
                keys: &[],
                selected,
                marked: None,
                border: true,
            }),
            bands: Bands::NONE,
            content: PaneContent::Lines(&[]),
        });
        list.push(Pane {
            id: ROWS,
            region: inner,
            layer: Layer::FLOAT_CONTENT,
            background: None,
            panel: None,
            bands: Bands::NONE,
            content: PaneContent::Lines(lines),
        });
    }
}
