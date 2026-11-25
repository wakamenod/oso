//! What an input method is composing (ADR 0069): where it is drawn and where its candidates go,
//! at the selected window's cursor, or in isearch on its line (ADR 0158).
//!
//! Must not depend on: `command`, `keymap`, or the window layer.

use super::Editor;
use crate::display_map::Preedit;
use crate::frame::{CellRect, PixelGrid, PixelRect};
use crate::motion;

impl Editor {
    /// Where the selected window's primary cursor is on the surface, in physical
    /// pixels, for the OS's input method to place its candidate window. `None` if
    /// it is out of view. In isearch, where its string goes on, on the echo row (ADR 0158).
    pub fn cursor_area(&self, grid: PixelGrid) -> Option<PixelRect> {
        if self.isearch().is_some() {
            return Some(grid.rect(self.search_cell()?));
        }
        let (window, buffer) = self.current();
        let display = buffer.display();
        let point = window.cursor_point(&display);
        let cells = motion::cursor_cells(&display, point);
        // With a preedit, its start: the cursor's cell with the preedit laid out (ADR 0104).
        let (row, origin) = window.cursor_place(&display, self.preedit.as_ref())?;
        let region = self.selected_region();
        let start = point.column.max(origin);
        let end =
            (point.column.saturating_add(cells.max(1))).min(origin.saturating_add(region.columns));
        (row < region.rows && start < end).then(|| {
            grid.rect(CellRect {
                row: region.row + row,
                column: region.column + (start - origin),
                rows: 1,
                columns: end - start,
            })
        })
    }

    /// Where the composition is drawn, for the input method (ADR 0077): the rectangle of its
    /// first cell as the window now scrolls, in physical pixels, and into `out`, emptied
    /// first, the x of each of its characters from that cell's left, then of its end, each
    /// kept inside the text area. `None`, with `out` empty, with no composition, one whose row
    /// is out of view, or one too long to place.
    pub fn preedit_layout(&self, grid: PixelGrid, out: &mut Vec<f32>) -> Option<PixelRect> {
        out.clear();
        let preedit = self.preedit.as_ref()?;
        if self.isearch().is_some() {
            let first = grid.rect(self.search_cell()?);
            let echo = self.frame.echo;
            let [right, _] = grid.cell(0, (echo.column + echo.columns) as i64);
            preedit.char_positions(grid.cell_width(), out);
            for position in out.iter_mut() {
                *position = position.min(right - first.x);
            }
            return (!out.is_empty()).then_some(first);
        }
        let (window, buffer) = self.current();
        let display = buffer.display();
        let point = window.cursor_point(&display);
        // Where it starts as its line is laid out with it in (ADR 0104). A preedit that wraps
        // keeps one rectangle, its start's, for now.
        let region = self.selected_region();
        let (row, origin) = window
            .cursor_place(&display, Some(preedit))
            .filter(|&(row, _)| row < region.rows)?;
        // Left of the text area when its start is scrolled off.
        let column = region.column as i64 + point.column as i64 - origin as i64;
        let [x, y] = grid.cell((region.row + row) as i64, column);
        preedit.char_positions(grid.cell_width(), out);
        if out.is_empty() {
            return None;
        }
        let [left, _] = grid.cell(0, region.column as i64);
        let [right, _] = grid.cell(0, (region.column + region.columns) as i64);
        for position in out.iter_mut() {
            *position = position.clamp(left - x, right - x);
        }
        let cell = grid.rect(CellRect {
            row: 0,
            column: 0,
            rows: 1,
            columns: 1,
        });
        Some(PixelRect { x, y, ..cell })
    }

    /// What an input method is composing in the selected window's text: none in isearch, whose
    /// line shows it (ADR 0158).
    pub(super) fn inline_preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref().filter(|_| self.isearch().is_none())
    }

    /// What an input method is composing, if anything (ADR 0069).
    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// Shows `preedit` at the selected window's primary cursor, or none, and follows
    /// it, or in isearch on its line, where the view stays (ADR 0158). A preedit with no text
    /// is none. Returns whether the view moved.
    pub fn set_preedit(&mut self, preedit: Option<Preedit>) -> bool {
        self.preedit = preedit.filter(|preedit| !preedit.text().is_empty());
        if self.preedit.is_none() {
            self.preedit_withheld = false;
        }
        self.note_secret_shown();
        self.isearch().is_none() && self.follow_cursor()
    }

    /// Whether the composition was open while a buffer that may hold secrets was shown, so the
    /// control socket leaves it out until it ends.
    pub fn preedit_withheld(&self) -> bool {
        self.preedit_withheld
    }
}
