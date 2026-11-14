//! A click on `*Sessions*` in the side window (ADR 0131, step 2b): point goes to the row under
//! it, which then does what `RET` does there, or folds when the click is on the row's fold
//! mark. It acts at once, whether the list was selected or not (the author, 2026-11-15). The
//! wheel is `wheel`'s, as in any window.
//!
//! Must not depend on: how a frame is drawn, beyond where a window's text is.

use oso_core::buffer::Mode;
use oso_core::editor::RowAction;
use oso_core::selection::SelectionSet;
use oso_core::window::WindowId;
use oso_platform::window::Position;

use crate::app::App;

/// The folds' marks, which begin a project's, a group's and a quiet row's line.
const FOLDS: [char; 2] = ['▾', '▸'];

impl App {
    /// A left press at `at` on `window`. True if it fell on the text of `*Sessions*` in the side
    /// window and was taken there; false leaves it to the press's other routes.
    pub(in crate::app) fn sessions_pressed(&mut self, window: WindowId, at: Position) -> bool {
        if self.editor.side_window() != Some(window) || self.oso_holds_input() {
            return false;
        }
        let shows_list = self
            .editor
            .window_by_id(window)
            .and_then(|shown| self.editor.buffer(shown.buffer()))
            .is_some_and(|buffer| buffer.mode() == Mode::Sessions);
        let cell = self
            .view
            .as_ref()
            .and_then(|view| view.render_state.pixel_grid().cell_at(at.x, at.y));
        let text = self
            .editor
            .frame()
            .regions(window)
            .map(|regions| regions.text);
        let (Some((row, column)), Some(text), true) = (cell, text, shows_list) else {
            return false;
        };
        if !(text.row..text.row + text.rows).contains(&row)
            || !(text.column..text.column + text.columns).contains(&column)
        {
            return false;
        }
        self.editor.select_window(window);
        let (shown, buffer) = self.editor.current();
        let lines = buffer.text();
        let line = lines.offset_to_point(shown.scroll().start).row + (row - text.row);
        if line >= lines.line_count() {
            // Below the last row: the list is selected, and nothing more.
            return true;
        }
        let (start, end) = (lines.line_start(line), lines.line_end(line));
        let mut chars = lines.chunks_in_range(start..end).flat_map(str::chars);
        let indent = chars.by_ref().take_while(|c| *c == ' ').count();
        let first = lines
            .chunks_in_range(start..end)
            .flat_map(str::chars)
            .nth(indent);
        let on_fold = column - text.column + shown.scroll().left == indent
            && first.is_some_and(|mark| FOLDS.contains(&mark));
        self.editor.current_mut().0.selections = SelectionSet::new(start);
        self.editor.follow_cursor();
        let action = if on_fold {
            RowAction::Fold
        } else {
            RowAction::Show
        };
        self.on_row(line, action);
        self.request_redraw();
        true
    }
}
