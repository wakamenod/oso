//! The bands of `*Spaces*` in the side window (ADR 0140): a section's row on its band, the
//! front worktree's rows on their tint, and, only while the pane is selected, the row at point
//! on the point's band with a bar in its first cell. The pane not selected draws no band for
//! point, so that whether it has the person's keys shows.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::window::{Screen, WindowId};

use crate::sessions::list::PaneBands;

/// What the side window fills behind the list's text, kept from frame to frame.
#[derive(Default)]
pub(super) struct PaneCells {
    pub(super) sections: Vec<CellRect>,
    pub(super) tint: Vec<CellRect>,
    pub(super) point: Vec<CellRect>,
    /// The first cell of the row at point, where the bar goes.
    pub(super) bar: Option<CellRect>,
}

impl PaneCells {
    /// Fills in what window `id`, as `screen` shows it, draws behind `*Spaces*` when it is the
    /// side window showing it: its sections' and front worktree's rows, from `bands`, and the
    /// row at point from `band`, the current line's band the editor gave, which it takes over.
    pub(super) fn fill(
        &mut self,
        (editor, bands): (&Editor, &PaneBands),
        (id, screen): (WindowId, &Screen),
        band: &mut Vec<CellRect>,
    ) {
        self.sections.clear();
        self.tint.clear();
        self.point.clear();
        self.bar = None;
        if editor.side_window() != Some(id) {
            return;
        }
        let shows_list = (editor.window_by_id(id))
            .and_then(|window| editor.buffer(window.buffer()))
            .is_some_and(|buffer| buffer.mode() == Mode::Sessions);
        let regions = editor.frame().windows().iter().find(|(w, _)| *w == id);
        let (true, Some((_, regions))) = (shows_list, regions) else {
            return;
        };
        let text = regions.text;
        for (index, row) in screen.rows().iter().take(text.rows).enumerate() {
            let at = text.row + index;
            if bands.sections.binary_search(&row.line).is_ok() {
                push_row(&mut self.sections, text, at);
            }
            if bands.tint.binary_search(&row.line).is_ok() {
                push_row(&mut self.tint, text, at);
            }
        }
        if editor.tiled().0.id() == id {
            self.bar = band.first().map(|rect| CellRect {
                columns: 1,
                ..*rect
            });
            self.point.append(band);
        }
        band.clear();
    }
}

/// Adds row `at` of the text area `text` to `rects`, as one more row of the last rectangle
/// when it is the row under it.
fn push_row(rects: &mut Vec<CellRect>, text: CellRect, at: usize) {
    match rects.last_mut() {
        Some(last) if last.row + last.rows == at => last.rows += 1,
        _ => rects.push(CellRect {
            row: at,
            rows: 1,
            ..text
        }),
    }
}
