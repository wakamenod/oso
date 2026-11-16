//! A click on `*Spaces*` in the side window (ADR 0131, step 2b; ADR 0140, Q5): point goes to
//! the row under it. On the selected list the row then does what `RET` does there; on a list
//! not selected the click selects it and does nothing more, so that the person sees it has
//! their keys first. A click on a row's fold mark folds, selected or not. The wheel is
//! `wheel`'s, as in any window.
//!
//! Must not depend on: how a frame is drawn, beyond where a window's text is.

use oso_core::buffer::Mode;
use oso_core::editor::{RowAction, SessionRequest};
use oso_core::selection::SelectionSet;
use oso_core::window::WindowId;
use oso_platform::window::Position;

use crate::app::App;
use crate::commands::agent_sessions::{FOLDS, row_mark};

impl App {
    /// A left press at `at` on `window`. True if it fell on the text of `*Spaces*` in the side
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
        let was_selected = self.editor.tiled().0.id() == window;
        self.editor.select_window(window);
        let (shown, buffer) = self.editor.current();
        let lines = buffer.text();
        let line = lines.offset_to_point(shown.scroll().start).row + (row - text.row);
        if line >= lines.line_count() {
            // Below the last row: the list is selected, and nothing more.
            return true;
        }
        let (start, end) = (lines.line_start(line), lines.line_end(line));
        // As the keys read a row: its mark past its indent and guides.
        let row = row_mark(lines.chunks_in_range(start..end).flat_map(str::chars));
        let on_fold = row.is_some_and(|(indent, mark)| {
            column - text.column + shown.scroll().left == indent && FOLDS.contains(&mark)
        });
        self.editor.current_mut().0.selections = SelectionSet::new(start);
        self.editor.follow_cursor();
        let action = if on_fold {
            RowAction::Fold
        } else if was_selected {
            RowAction::Show
        } else {
            // The list was not selected: selecting it and moving point is all (Q5).
            self.request_redraw();
            return true;
        };
        // As the row's key asks, so that the list renders again after it as after a key.
        self.editor
            .request_session(SessionRequest::Row { line, action });
        self.request_redraw();
        true
    }
}
