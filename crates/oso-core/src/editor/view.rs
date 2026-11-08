//! The editor's frame and its selected window's view: laying out the frame,
//! scrolling to follow the cursor and for the wheel, and the cells its regions take
//! (ADR 0040, ADR 0057, ADR 0058, ADR 0071).
//!
//! Must not depend on: `command` or `keymap`, or the host's window; the rules for
//! scrolling are pure functions elsewhere, and this applies them to the state.

use super::Editor;
use crate::display_map::{DisplayPoint, Preedit};
use crate::frame::{self, CellRect, FrameLayout, FrameSize, PixelGrid, PixelRect};
use crate::motion;
use crate::selection::{MarkActive, SelectionSet};
use crate::window::{self, Scroll, ViewSize};

impl Editor {
    /// Lays out a frame of `size` (ADR 0057). The windows take its text area, and
    /// the selected one then scrolls to show its primary cursor. Rows it gained
    /// also show the text above any empty space below the buffer's last row.
    /// Returns whether the view moved.
    pub fn set_frame_size(&mut self, size: FrameSize) -> bool {
        self.frame = frame::layout(size);
        let view = self.frame.window.text.view_size();
        let tiled = self.tiled_index();
        let added = view.rows.saturating_sub(self.windows[tiled].size().rows);
        let count = self.tiled_count();
        for window in &mut self.windows[..count] {
            window.set_size(view);
        }
        // A prompt's window takes where its answer goes, and the window beneath it
        // follows its own cursor too, as the frame changed under both.
        self.relayout_prompt();
        let beneath = tiled != self.selected && self.follow_window(tiled);
        let followed = self.follow_cursor();
        let filled = self.fill_below_last_row(tiled, added);
        followed || filled || beneath
    }

    /// Pulls the selected window's top row back by at most `added` rows, and no
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
        let scroll = window.scroll();
        let bottom = buffer
            .display()
            .row_count()
            .saturating_sub(window.size().rows);
        let top = scroll.top.saturating_sub(added).max(bottom.min(scroll.top));
        window.set_scroll(Scroll { top, ..scroll });
        top != scroll.top
    }

    /// Lays out the frame whose text area is `size`, for tests that think in text
    /// rows. Returns whether the view moved.
    pub fn set_view_size(&mut self, size: ViewSize) -> bool {
        self.set_frame_size(FrameSize::holding(size))
    }

    /// The frame's regions.
    pub fn frame(&self) -> FrameLayout {
        self.frame
    }

    /// Where the selected window's primary cursor is on the surface, in physical
    /// pixels, for the OS's input method to place its candidate window. `None` if
    /// it is out of view.
    pub fn cursor_area(&self, grid: PixelGrid) -> Option<PixelRect> {
        let (window, buffer) = self.current();
        let display = buffer.display();
        let point = window.cursor_point(&display);
        let cells = motion::cursor_cells(&display, point);
        let region = match self.prompt_layout() {
            Some(layout) => layout.input,
            None => self.frame.window.text,
        };
        let cell = frame::cursor_rect(region, window.scroll(), point, cells)?;
        Some(grid.rect(cell))
    }

    /// Hides the selected window's regions and keeps its marks, as `C-g` does, first
    /// in the chain of what it cancels (ADR 0030, ADR 0071). Returns whether a region
    /// was shown, so that a caller can stop there and ask for a frame.
    pub fn deactivate_mark(&mut self) -> bool {
        let selections = &mut self.windows[self.selected].selections;
        selections.set_mark_active(MarkActive::Inactive).is_active()
    }

    /// The cells of the selected window's text area that its regions take, into
    /// `out`, which is emptied first, and none while they are hidden (ADR 0071). A
    /// preedit pushes them as it pushes the text.
    pub fn region_cells(&self, out: &mut Vec<CellRect>) {
        out.clear();
        // The tiled window's, also beneath a prompt, whose text area these cells are in.
        let (window, buffer) = self.tiled();
        if !window.selections.mark_active().is_active() {
            return;
        }
        let display = buffer.display();
        let preedit =
            (self.preedit.as_ref()).map(|preedit| (window.cursor_point(&display), preedit.cells()));
        let (text, scroll) = (self.frame.window.text, window.scroll());
        frame::region_cells(text, scroll, &display, &window.selections, preedit, out);
    }

    /// The cells of the innermost prompt's answer that its regions take, into `out`, which
    /// is emptied first, and none while they are hidden or no prompt is open (ADR 0076). No
    /// input method composes in a prompt, so no preedit pushes them.
    pub fn prompt_region_cells(&self, out: &mut Vec<CellRect>) {
        out.clear();
        let (Some(_), Some(layout)) = (self.prompt(), self.prompt_layout()) else {
            return;
        };
        let window = &self.windows[self.selected];
        if !window.selections.mark_active().is_active() {
            return;
        }
        let display = self.shown(window).display();
        let scroll = window.scroll();
        frame::region_cells(
            layout.input,
            scroll,
            &display,
            &window.selections,
            None,
            out,
        );
    }

    /// What an input method is composing, if anything (ADR 0069).
    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// Shows `preedit` at the selected window's primary cursor, or none, and follows
    /// it. A preedit with no text is none. Returns whether the view moved.
    pub fn set_preedit(&mut self, preedit: Option<Preedit>) -> bool {
        self.preedit = preedit.filter(|preedit| !preedit.text().is_empty());
        self.follow_cursor()
    }

    /// Scrolls the selected window by the fewest rows and columns that show
    /// its primary cursor (`scroll-conservatively` 101). The host calls it
    /// after every command. With a preedit, it shows the preedit's start up to the
    /// cursor box, or the box alone if both do not fit (ADR 0069). Returns whether
    /// the view moved.
    pub fn follow_cursor(&mut self) -> bool {
        self.follow_window(self.selected)
    }

    /// Scrolls the window at `index` as [`Editor::follow_cursor`] does the selected one.
    /// A preedit counts only in the selected window, where it shows.
    pub(super) fn follow_window(&mut self, index: usize) -> bool {
        // The window is borrowed from the whole editor, so the preedit steps out.
        let preedit = if index == self.selected {
            self.preedit.take()
        } else {
            None
        };
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let point = window.cursor_point(&display);
        let (cursor, cells) = motion::cursor_box(&display, point, preedit.as_ref());
        let span = cursor.column - point.column + cells;
        let (point, cells) = if span <= window.size().columns {
            (point, span)
        } else {
            (DisplayPoint::new(point.row, cursor.column), cells)
        };
        let scroll = window::follow(window.scroll(), window.size(), point, cells);
        let moved = scroll != window.scroll();
        window.set_scroll(scroll);
        if preedit.is_some() {
            self.preedit = preedit;
        }
        moved
    }

    /// Puts back the selected window's cursors and view, saved together as a
    /// state handoff does, then scrolls as little as shows its primary cursor.
    pub fn restore_positions(&mut self, selections: SelectionSet, scroll: Scroll) {
        // The tiled window, also beneath a prompt, whose own window shows only its buffer.
        let index = self.tiled_index();
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
        let (window, buffer) = self.window_mut(self.tiled_index());
        let display = buffer.display();
        let row_count = display.row_count();
        let scroll = window.scroll();
        let top = window::wheel(scroll.top, window.size(), row_count, rows);
        if cursors_stay {
            window.set_scroll(Scroll { top, ..scroll });
            return top != scroll.top;
        }
        window.set_top(top, &display)
    }

    /// Scrolls the selected window's view by `columns` (toward the ends of lines
    /// if positive), for the mouse wheel and trackpad, as far as the rows in view
    /// need. While a prompt is open, this is the window beneath it. Cursors stay, even out of view, and the next command brings the view
    /// back. A short row may not reach the view at all, so no place in view would
    /// do. Returns whether the view moved.
    pub fn scroll_view_sideways(&mut self, columns: isize) -> bool {
        let (window, buffer) = self.window_mut(self.tiled_index());
        let display = buffer.display();
        let scroll = window.scroll();
        let size = window.size();
        // How wide a row must be to go all the way, so that the search can stop
        // there. Going left needs no row at all.
        let wanted = scroll
            .left
            .saturating_add(columns.unsigned_abs())
            .saturating_add(size.columns);
        let mut widest = 0;
        if columns > 0 {
            for row in scroll.top..scroll.top.saturating_add(size.rows) {
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
