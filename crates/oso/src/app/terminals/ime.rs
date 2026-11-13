//! Where the input method composes in a terminal (ADR 0120): at the program's cursor cell,
//! even while the program hides the cursor, as Claude Code's input box does.
//!
//! Must not depend on: how a frame is drawn, beyond the grid of cells in pixels.

use super::App;

impl App {
    /// The selected terminal's cursor cell in physical pixels, within its window's text, for
    /// the input method's candidate window.
    pub(in crate::app) fn terminal_cursor_area(
        &self,
        grid: oso_core::frame::PixelGrid,
    ) -> Option<oso_core::frame::PixelRect> {
        let window = self.editor.window();
        let hosted = self.terminals.map.get(window.buffer())?;
        let text = self.editor.frame().regions(window.id())?.text;
        let (row, column) = hosted.grid.cursor_cell();
        let (row, column) = (usize::from(row), usize::from(column));
        (row < text.rows && column < text.columns).then(|| {
            grid.rect(oso_core::frame::CellRect {
                row: text.row + row,
                column: text.column + column,
                rows: 1,
                columns: 1,
            })
        })
    }

    /// Where a composition in the selected terminal is drawn: its first cell, at the cursor,
    /// and into `out` the x of each of its characters from that cell's left, then of its end.
    pub(in crate::app) fn terminal_preedit_layout(
        &self,
        grid: oso_core::frame::PixelGrid,
        out: &mut Vec<f32>,
    ) -> Option<oso_core::frame::PixelRect> {
        out.clear();
        let preedit = self.editor.preedit()?;
        let first = self.terminal_cursor_area(grid)?;
        let width = first.width;
        let mut x = 0.0_f32;
        for cluster in oso_core::display_map::text_clusters(preedit.text(), 0) {
            for _ in cluster.text.chars() {
                out.push(x);
            }
            x += width * cluster.cells as f32;
        }
        out.push(x);
        Some(first)
    }
}
