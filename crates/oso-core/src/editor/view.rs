//! The editor's frame and its selected window's view: laying out the frame,
//! scrolling to follow the cursor and for the wheel, and the cells its regions take
//! (ADR 0040, ADR 0057, ADR 0058, ADR 0071).
//!
//! Must not depend on: `command` or `keymap`, or the host's window; the rules for
//! scrolling are pure functions elsewhere, and this applies them to the state.

use super::Editor;
use crate::buffer::Buffer;
use crate::display_map::{DisplayPoint, DisplaySnapshot, Preedit};
use crate::frame::{self, CellRect, FrameLayout, FrameSize, PixelGrid, PixelRect, WindowRegions};
use crate::motion;
use crate::motion::Edge;
use crate::selection::{MarkActive, SelectionSet};
use crate::window::{self, Recenter, Screen, Scroll, ViewSize, ViewTop, Window, WindowId};

impl Editor {
    /// Lays out a frame of `size` (ADR 0057), with the gutter line numbers need (ADR 0097).
    /// The windows take its text area, and
    /// the selected one then scrolls to show its primary cursor. Rows it gained
    /// also show the text above any empty space below the buffer's last row.
    /// Returns whether the view moved.
    pub fn set_frame_size(&mut self, size: FrameSize) -> bool {
        // A resize starts each wrap's budget (ADR 0103).
        self.refill_wraps();
        self.lay_out(size)
    }

    /// [`Editor::set_frame_size`] within what is left of the wraps' budget, for a gutter that
    /// gained a digit in a command, or a window tree that changed, which a frame laid out again
    /// does not start anew. Each tiled window takes its own text area, with its own gutter
    /// (ADR 0112), and the selected one never loses its cells.
    pub(super) fn lay_out(&mut self, size: FrameSize) -> bool {
        let count = self.tiled_count();
        let gutters: Vec<(WindowId, usize)> = self.windows[..count]
            .iter()
            .map(|window| (window.id(), self.gutter_columns(window)))
            .collect();
        let numbers = self.aids.line_numbers;
        for (window, &(_, gutter)) in self.windows[..count].iter_mut().zip(&gutters) {
            window.set_gutter_floor(numbers.then_some((window.buffer(), gutter)));
        }
        let tiled = self.tiled_index();
        let keep = self.windows[tiled].id();
        frame::layout_into(&mut self.frame, size, &self.splits, keep, |id| {
            gutters
                .iter()
                .find_map(|&(other, gutter)| (other == id).then_some(gutter))
                .unwrap_or(0)
        });
        let mut added = Vec::with_capacity(count);
        for window in &mut self.windows[..count] {
            let view = self
                .frame
                .regions(window.id())
                .map_or(ViewSize::new(1, 1), |regions| regions.text.view_size());
            added.push(view.rows.saturating_sub(window.size().rows));
            window.set_size(view);
        }
        // A prompt's window takes where its answer goes, and the window beneath it
        // follows its own cursor too, as the frame changed under both.
        self.relayout_prompt();
        let beneath = tiled != self.selected && self.follow_window(tiled);
        let mut followed = self.follow_cursor();
        // The frame changed under every window, so each follows its own cursor.
        for index in 0..count {
            if index != tiled {
                followed |= self.follow_window(index);
            }
        }
        if tiled == self.selected {
            self.pass_wrap_budget(tiled);
        }
        let mut filled = false;
        for (index, added) in added.into_iter().enumerate() {
            filled |= self.fill_below_last_row(index, added);
        }
        followed || filled || beneath
    }

    /// Pulls the top row of the window at `index` back by at most `added` rows, and no
    /// further than puts the last row at the bottom, the wheel's limit
    /// ([`window::wheel`]). A short window moved the top row to the cursor, and
    /// only this moves it back. A view past the end otherwise stays, as the wheel
    /// leaves it (ADR 0040), and it moves no more than the window grew, so it
    /// stays steady while an edge is dragged. The primary cursor stays in view: it
    /// is on or below the old top, and when the top moves, the view still reaches
    /// the last row. Other cursors keep their places, so this does not use
    /// [`Window::set_top`]. Returns whether the view moved.
    fn fill_below_last_row(&mut self, index: usize, added: usize) -> bool {
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let top = window.top_row(&display);
        let bottom = window
            .row_count(&display)
            .saturating_sub(window.size().rows);
        let row = top.saturating_sub(added).max(bottom.min(top));
        if row == top {
            return false;
        }
        let left = window.scroll().left;
        window.set_view_top(ViewTop { row, left }, &display)
    }

    /// Lays out the frame whose text area is `size` while line numbers are off, for tests
    /// that think in text rows. Returns whether the view moved.
    pub fn set_view_size(&mut self, size: ViewSize) -> bool {
        self.set_frame_size(FrameSize::holding(size))
    }

    /// The view row at the top of the selected window.
    pub fn top_row(&self) -> usize {
        let (window, buffer) = self.current();
        window.top_row(&buffer.display())
    }

    /// The frame's regions.
    pub fn frame(&self) -> &FrameLayout {
        &self.frame
    }

    /// `window`'s regions, if it is a tiled window (ADR 0112).
    pub fn regions(&self, window: WindowId) -> Option<&WindowRegions> {
        self.frame.regions(window)
    }

    /// The regions of the selected tiled window, or of the one beneath the prompts: none
    /// while it has no room in the frame.
    pub fn tiled_regions(&self) -> WindowRegions {
        let regions = self.regions(self.tiled().0.id());
        debug_assert!(regions.is_some(), "the tiled window is in the layout");
        regions.copied().unwrap_or(WindowRegions::EMPTY)
    }

    /// Where the selected window's primary cursor is on the surface, in physical
    /// pixels, for the OS's input method to place its candidate window. `None` if
    /// it is out of view.
    pub fn cursor_area(&self, grid: PixelGrid) -> Option<PixelRect> {
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

    /// Hides the selected window's regions and keeps its marks, as `C-g` does, first
    /// in the chain of what it cancels (ADR 0030, ADR 0071). Returns whether a region
    /// was shown, so that a caller can stop there and ask for a frame.
    pub fn deactivate_mark(&mut self) -> bool {
        let selections = &mut self.windows[self.selected].selections;
        selections.set_mark_active(MarkActive::Inactive).is_active()
    }

    /// The window `id` in the tree, the buffer it shows and its regions, if it has them.
    pub(super) fn window_in_tree(&self, id: WindowId) -> Option<(&Window, &Buffer, WindowRegions)> {
        let regions = *self.regions(id)?;
        let window = self.window_by_id(id)?;
        Some((window, self.shown(window), regions))
    }

    /// As [`Editor::window_in_tree`], for what only the selected window draws: the region, the
    /// current line's band, the flash and isearch's matches (ADR 0112). `None` for any other
    /// window, and for every tiled window beneath a prompt, which is drawn as not selected.
    pub(super) fn selected_in_tree(
        &self,
        id: WindowId,
    ) -> Option<(&Window, &Buffer, WindowRegions)> {
        let selected = self.prompts.is_empty() && id == self.tiled_id();
        self.window_in_tree(id).filter(|_| selected)
    }

    /// The cells of the text area of the window `id` that its regions take, as `screen` shows
    /// it ([`Editor::screen_of`]), into `out`, which is emptied first, and none while they are
    /// hidden (ADR 0071). Only the selected window has them, and none beneath a prompt
    /// (ADR 0112). A preedit pushes them as it pushes the text.
    pub fn region_cells(&self, id: WindowId, screen: &Screen, out: &mut Vec<CellRect>) {
        out.clear();
        let Some((window, buffer, regions)) = self.selected_in_tree(id) else {
            return;
        };
        if !window.selections.mark_active().is_active() {
            return;
        }
        frame::region_cells(
            regions.text,
            screen,
            &buffer.display(),
            &window.selections,
            out,
        );
    }

    /// The cells of the text area of the window `id` that the boxes of its cursors other than
    /// the primary take, as `screen` shows it ([`Editor::screen_of`]), into `out`, which is
    /// emptied first (ADR 0091). A preedit pushes them as it pushes the text.
    pub fn other_cursor_cells(&self, id: WindowId, screen: &Screen, out: &mut Vec<CellRect>) {
        let Some((window, buffer, regions)) = self.window_in_tree(id) else {
            out.clear();
            return;
        };
        frame::other_cursor_cells(
            regions.text,
            screen,
            &buffer.display(),
            &window.selections,
            out,
        );
    }

    /// The cells of the innermost prompt's answer that its regions take, as `screen` shows it
    /// ([`Editor::prompt_screen`]), into `out`, which is emptied first, and none while they are
    /// hidden or no prompt is open (ADR 0076). A preedit there pushes them as it pushes the
    /// text.
    pub fn prompt_region_cells(&self, screen: &Screen, out: &mut Vec<CellRect>) {
        out.clear();
        let (Some(_), Some(layout)) = (self.prompt(), self.prompt_layout()) else {
            return;
        };
        let window = &self.windows[self.selected];
        if !window.selections.mark_active().is_active() {
            return;
        }
        let display = self.shown(window).display();
        frame::region_cells(layout.input, screen, &display, &window.selections, out);
    }

    /// What an input method is composing, if anything (ADR 0069).
    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// Shows `preedit` at the selected window's primary cursor, or none, and follows
    /// it. A preedit with no text is none. Returns whether the view moved.
    pub fn set_preedit(&mut self, preedit: Option<Preedit>) -> bool {
        self.preedit = preedit.filter(|preedit| !preedit.text().is_empty());
        if self.preedit.is_none() {
            self.preedit_withheld = false;
        }
        self.note_secret_shown();
        self.follow_cursor()
    }

    /// Whether the composition was open while a buffer that may hold secrets was shown, so the
    /// control socket leaves it out until it ends.
    pub fn preedit_withheld(&self) -> bool {
        self.preedit_withheld
    }

    /// Scrolls the selected window by the fewest rows and columns that show
    /// its primary cursor (`scroll-conservatively` 101). The host calls it
    /// after every command. With a preedit, it shows the preedit's start up to the
    /// cursor box, or the box alone if both do not fit (ADR 0069). Returns whether
    /// a view moved.
    ///
    /// Each other tiled window that an edit moved, its cursor out of view or its top past the
    /// end of the text, then follows its own primary cursor too (ADR 0112), sharing what the
    /// selected window left of the command's wrap budget. Not while a prompt is open: the
    /// wheel scrolls the window beneath it and leaves its cursors (ADR 0072); they follow once
    /// it closes.
    pub fn follow_cursor(&mut self) -> bool {
        // So does the preview's wrap, which a command may have moved (ADR 0100).
        self.fit_preview();
        let mut moved = self.follow_window(self.selected);
        if self.prompts.is_empty() && !self.moved_windows.is_empty() {
            // By position, so the list keeps its room from command to command.
            for at in 0..self.moved_windows.len() {
                let id = self.moved_windows[at];
                if let Some(index) = self.window_index(id).filter(|&i| i != self.selected) {
                    moved |= self.follow_window(index);
                }
            }
            self.moved_windows.clear();
            // What is left goes back to the selected window, for what the command does next.
            self.pass_wrap_budget(self.selected);
        }
        moved
    }

    /// Scrolls the window at `index` as [`Editor::follow_cursor`] does the selected one.
    /// A preedit counts only in the selected window, where it shows.
    /// Its wrap is brought in step first: on or off as its buffer is, at its width.
    /// It takes what is left of the command's wrap budget (ADR 0112).
    pub(super) fn follow_window(&mut self, index: usize) -> bool {
        self.sync_wrap(index);
        // The window is borrowed from the whole editor, so the preedit steps out.
        let preedit = if index == self.selected {
            self.preedit.take()
        } else {
            None
        };
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let before = window.scroll();
        // The cursor's row, then the rows in view once there, wrapped if provisional
        // (ADR 0103); a top they moved is found again.
        window.realize(&display, window.selections.primary().cursor, 0);
        follow_in(window, &display, preedit.as_ref());
        if window.realize_view(&display) {
            follow_in(window, &display, preedit.as_ref());
        }
        let moved = window.scroll() != before;
        if preedit.is_some() {
            self.preedit = preedit;
        }
        moved
    }

    /// Puts back the selected window's cursors and view, saved together as a
    /// state handoff does, then scrolls as little as shows its primary cursor.
    pub fn restore_positions(&mut self, selections: SelectionSet, scroll: Scroll) {
        // Against the gutter the text loaded so far needs (ADR 0097).
        self.fit_gutter();
        // The tiled window, also beneath a prompt, whose own window shows only its buffer.
        let index = self.tiled_index();
        self.sync_wrap(index);
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        window.restore_positions(selections, scroll, &display);
        self.follow_window(index);
    }

    /// Scrolls the selected window's view by `rows` (down toward the end
    /// if positive), for the mouse wheel and trackpad. Cursors left outside
    /// the view move to its nearest row, as in Emacs, but not while a preedit is
    /// shown: the commit goes where the composition started, and the next command
    /// brings the view back (ADR 0069). Returns whether the view moved.
    ///
    /// While a prompt is open, this scrolls the window beneath it, and its cursors stay, so
    /// that what the prompt's command acts on does not move (ADR 0072).
    pub fn scroll_view(&mut self, rows: isize) -> bool {
        let cursors_stay = self.preedit.is_some() || !self.prompts.is_empty();
        self.refill_wraps();
        let (window, buffer) = self.window_mut(self.tiled_index());
        let display = buffer.display();
        let before = window.scroll();
        // The rows the wheel reaches, wrapped first if provisional (ADR 0103).
        let count = rows.unsigned_abs();
        if rows < 0 {
            // The rows just above the top first, from an anchor before it, so that a
            // provisional row there does not take the top to its line's start.
            window.realize(&display, window.scroll().start.saturating_sub(1), 0);
            let row = window.top_row(&display).saturating_sub(count);
            let at = window.rows(&display).start(row);
            window.realize(&display, at, count);
        } else {
            let reach = window.size().rows.saturating_add(count);
            window.realize(&display, window.scroll().start, reach);
        }
        let top = window.top_row(&display);
        let next = window::wheel(top, window.size(), window.row_count(&display), rows);
        if cursors_stay {
            let left = before.left;
            window.set_view_top(ViewTop { row: next, left }, &display);
        } else {
            window.set_top(next, &display);
        }
        window.realize_view(&display);
        window.scroll() != before
    }

    /// Scrolls the selected window by `lines` view rows, the text up, toward the end, if
    /// positive, as Emacs's `scroll-up` does, and moves cursors that would leave it to the
    /// start of its nearest row (ADR 0040). The rows it reaches are wrapped first if they are
    /// provisional (ADR 0103). Returns whether the view moved.
    ///
    /// # Errors
    ///
    /// The edge it cannot scroll past, with nothing moved.
    pub fn scroll_lines(&mut self, lines: isize) -> Result<bool, Edge> {
        let (window, buffer) = self.current_mut();
        let display = buffer.display();
        let count = lines.unsigned_abs();
        if lines >= 0 {
            let reach = window.size().rows.saturating_add(count);
            window.realize(&display, window.scroll().start, reach);
        } else {
            // As the wheel does going up.
            window.realize(&display, window.scroll().start.saturating_sub(1), 0);
            let row = window.top_row(&display).saturating_sub(count);
            let at = window.rows(&display).start(row);
            window.realize(&display, at, count);
        }
        let (top, row_count) = (window.top_row(&display), window.row_count(&display));
        let next = if lines >= 0 {
            window::scroll_up(top, row_count, count)
        } else {
            window::scroll_down(top, count)
        }?;
        let moved = window.set_top(next, &display);
        window.realize_view(&display);
        Ok(moved)
    }

    /// Scrolls the selected window so that its primary cursor's view row is where `target`
    /// says, as `C-l` does (ADR 0058). Returns whether the view moved.
    pub fn recenter(&mut self, target: Recenter) -> bool {
        let (window, buffer) = self.current_mut();
        let display = buffer.display();
        let cursor = window.selections.primary().cursor;
        window.realize(&display, cursor, window.size().rows);
        let row = window.cursor_row(&display);
        let top = window::recenter_top(row, window.size(), target);
        let moved = window.set_top(top, &display);
        window.realize_view(&display);
        moved
    }

    /// Scrolls the selected window's view by `columns` (toward the ends of lines
    /// if positive), for the mouse wheel and trackpad, as far as the rows in view
    /// need. While a prompt is open, this is the window beneath it. Cursors stay, even out of view, and the next command brings the view
    /// back. A short row may not reach the view at all, so no place in view would
    /// do. Returns whether the view moved.
    pub fn scroll_view_sideways(&mut self, columns: isize) -> bool {
        let (window, buffer) = self.window_mut(self.tiled_index());
        if window.wrap().is_some() {
            // Nothing scrolls sideways while the buffer wraps (ADR 0100).
            return false;
        }
        let display = buffer.display();
        let scroll = window.scroll();
        // Unwrapped, as nothing scrolls sideways while wrapped: a view row is a line.
        let top = window.top_row(&display);
        let size = window.size();
        // How wide a row must be to go all the way, so that the search can stop
        // there. Going left needs no row at all.
        let wanted = scroll
            .left
            .saturating_add(columns.unsigned_abs())
            .saturating_add(size.columns);
        let mut widest = 0;
        if columns > 0 {
            for row in top..top.saturating_add(size.rows) {
                widest = widest.max(display.row_cells(row));
                if widest >= wanted {
                    break;
                }
            }
        }
        let left = window::wheel_sideways(scroll.left, size, widest, columns);
        window.set_scroll(Scroll { left, ..scroll });
        left != scroll.left
    }
}

/// Scrolls `window` by the fewest rows and columns that show its primary cursor, its box
/// pushed by `preedit` (ADR 0069). While wrapped, rows are view rows and the left column 0.
fn follow_in(window: &mut Window, display: &DisplaySnapshot, preedit: Option<&Preedit>) {
    let point = window.cursor_point(display);
    let (cursor, cells) = motion::cursor_box(display, point, preedit);
    let span = cursor.column - point.column + cells;
    let (point, cells) = if window.wrap().is_some() {
        // While composing, the caret's row as the preedit wraps (ADR 0104).
        let row = preedit
            .and_then(|preedit| window.caret_row(display, preedit))
            .unwrap_or_else(|| window.cursor_row(display));
        (DisplayPoint::new(row, 0), 1)
    } else if span <= window.size().columns {
        (point, span)
    } else {
        (DisplayPoint::new(point.row, cursor.column), cells)
    };
    let top = ViewTop {
        row: window.top_row(display),
        left: window.scroll().left,
    };
    let next = window::follow(top, window.size(), point, cells);
    if next != top {
        window.set_view_top(next, display);
    }
}
