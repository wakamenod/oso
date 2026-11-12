//! The tiled windows of the frame (ADR 0112): splitting, deleting, selecting and resizing
//! them, and the order they were selected in.
//!
//! Must not depend on: `command`, `keymap` or the host's window. The tree's rules are pure
//! functions of the tree and the cells, in the frame's module; this applies them to the
//! windows and lays the frame out again after each change.
//!
//! Tiled windows come first in [`Editor::windows`], in the order they were made, and the
//! prompts' after them, so a split puts its new window just before the prompts'.

use std::fmt;

use super::Editor;
use crate::frame::{Side, Splits, TooSmall, Way};
use crate::window::WindowId;

/// `delete-window` refused: the window is the only one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoleWindow;

impl fmt::Display for SoleWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Attempt to delete the sole window")
    }
}

impl std::error::Error for SoleWindow {}

/// `windmove-*` found no window on that side of the selected one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoWindow(pub Side);

impl fmt::Display for NoWindow {
    /// As Emacs's `windmove` says it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = match self.0 {
            Side::Left => "left",
            Side::Right => "right",
            Side::Up => "up",
            Side::Down => "down",
        };
        write!(f, "No window {side} from selected window")
    }
}

impl std::error::Error for NoWindow {}

/// A resize moved nothing: no window on that side could give or take a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CannotResize {
    Enlarge,
    Shrink,
}

impl fmt::Display for CannotResize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Enlarge => "Cannot enlarge the window",
            Self::Shrink => "Cannot shrink the window",
        })
    }
}

impl std::error::Error for CannotResize {}

impl Editor {
    /// The position of the window named `id` in [`Editor::windows`].
    pub(super) fn window_index(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|window| window.id() == id)
    }

    /// Tells the tree the gutter its smallest window counts: the narrowest while line numbers
    /// are on, whatever any buffer's line count (ADR 0112).
    fn set_split_gutter(&mut self) {
        let gutter = if self.aids.line_numbers {
            super::gutter::NARROWEST
        } else {
            0
        };
        self.splits.set_gutter(gutter);
    }

    /// The selected tiled window's name, or the name of the one beneath the prompts.
    pub(super) fn tiled_id(&self) -> WindowId {
        self.windows[self.tiled_index()].id()
    }

    /// Splits the selected tiled window in two, stacked with [`Way::Down`] or side by side
    /// with [`Way::Across`], the new window below or at the right. It shows what the window
    /// does, with the primary cursor alone ([`Window::split_off`](crate::window::Window)).
    /// The original stays selected, or with `select` the new one is. Returns the new
    /// window's name. Ends isearch.
    ///
    /// # Errors
    ///
    /// [`TooSmall`] when either half would be smaller than a window may be. Nothing changes.
    pub fn split_window(&mut self, way: Way, select: bool) -> Result<WindowId, TooSmall> {
        let at = self.tiled_index();
        let id = WindowId::new(self.next_window);
        self.set_split_gutter();
        let mut splits = self.splits.clone();
        splits.split(self.windows[at].id(), way, id, &self.frame)?;
        self.isearch_exit();
        self.splits = splits;
        self.next_window += 1;
        let mut window = self.windows[at].split_off(id);
        // Its wrap spends from the command's one budget, not one of its own (ADR 0112).
        if let Some(wrap) = window.wrap_map_mut() {
            wrap.take_budget();
        }
        let count = self.tiled_count();
        self.windows.insert(count, window);
        if self.selected >= count {
            self.selected += 1;
        }
        // Selected the least recently, until it is.
        self.recent.push(id);
        self.lay_out(self.frame.size());
        if select {
            self.select_window(id);
        }
        Ok(id)
    }

    /// Deletes the selected tiled window, and selects the window selected most recently before
    /// it. Its cells go to its neighbour, as [`Splits::remove`] says. Ends isearch.
    ///
    /// # Errors
    ///
    /// [`SoleWindow`] for the only window. Nothing changes.
    pub fn delete_window(&mut self) -> Result<(), SoleWindow> {
        let id = self.tiled_id();
        let mut splits = self.splits.clone();
        if !splits.remove(id, &self.frame) {
            return Err(SoleWindow);
        }
        self.isearch_exit();
        self.splits = splits;
        self.drop_window(id);
        self.lay_out(self.frame.size());
        Ok(())
    }

    /// Deletes every tiled window but the selected one, which takes the whole tiled area. Does
    /// nothing with one window. Ends isearch. Returns whether it deleted any.
    pub fn delete_other_windows(&mut self) -> bool {
        let keep = self.tiled_id();
        let others: Vec<WindowId> = self.splits.iter().filter(|id| *id != keep).collect();
        if others.is_empty() {
            return false;
        }
        self.isearch_exit();
        self.splits = Splits::new(keep);
        for id in others {
            self.drop_window(id);
        }
        self.lay_out(self.frame.size());
        true
    }

    /// Takes the window `id` out of the editor, once the tree no longer holds it, and selects
    /// the window selected most recently instead of it if it was selected.
    fn drop_window(&mut self, id: WindowId) {
        let Some(index) = self.window_index(id) else {
            return;
        };
        // A buffer that may hold secrets goes off the screen with it (ADR 0063).
        if self.shown(&self.windows[index]).is_secret() {
            self.note_secret_shown();
            self.window_moves_on();
        }
        let was_tiled = self.tiled_index() == index;
        self.windows.remove(index);
        self.recent.retain(|other| *other != id);
        if self.selected > index {
            self.selected -= 1;
        }
        if was_tiled && let Some(&next) = self.recent.first() {
            self.make_tiled(next);
        }
    }

    /// Makes the window `id` the selected tiled window, or the one beneath the prompts while
    /// one is open, and the most recently selected.
    fn make_tiled(&mut self, id: WindowId) {
        let Some(index) = self.window_index(id) else {
            return;
        };
        match self.prompts.first_mut() {
            Some(outermost) => outermost.beneath = id,
            None => self.selected = index,
        }
        self.recent.retain(|other| *other != id);
        self.recent.insert(0, id);
    }

    /// Selects the tiled window `id`. Does nothing for a window that is not tiled, or that is
    /// selected already. Ends isearch, which runs in the selected window only.
    pub fn select_window(&mut self, id: WindowId) {
        if id == self.tiled_id() || !self.splits.contains(id) {
            return;
        }
        self.isearch_exit();
        let hidden = !self
            .frame
            .regions(id)
            .is_some_and(crate::frame::WindowRegions::has_cells);
        self.make_tiled(id);
        // The selected window always has cells, so a frame too small for every window lays
        // out again around it.
        if hidden {
            self.lay_out(self.frame.size());
        }
    }

    /// Selects the window `count` after the selected one in the tree's order, wrapping
    /// around, or before it if negative, as `C-x o` does. Windows the frame has no room for
    /// are skipped.
    pub fn other_window(&mut self, count: isize) {
        let me = self.tiled_id();
        let shown = |(_, regions): &&(WindowId, crate::frame::WindowRegions)| regions.has_cells();
        let windows = self.frame.windows();
        let len = windows.iter().filter(shown).count();
        let Some(at) = windows.iter().filter(shown).position(|(id, _)| *id == me) else {
            return;
        };
        let Ok(signed) = isize::try_from(len) else {
            return;
        };
        let step = usize::try_from(count.rem_euclid(signed.max(1))).unwrap_or(0);
        let next = (at + step) % len.max(1);
        if let Some((id, _)) = windows.iter().filter(shown).nth(next) {
            let id = *id;
            self.select_window(id);
        }
    }

    /// Selects the window that touches the selected one on `side`, at the row or column of
    /// its primary cursor, as Emacs's `windmove-left` and the others do.
    ///
    /// # Errors
    ///
    /// [`NoWindow`] with none there. Nothing changes.
    pub fn windmove(&mut self, side: Side) -> Result<(), NoWindow> {
        let id = self.tiled_id();
        let text = self.regions(id).ok_or(NoWindow(side))?.text;
        let (window, buffer) = self.tiled();
        let display = buffer.display();
        let column = window.cursor_point(&display).column;
        // The cursor's cell, or the text area's first while it is out of view.
        let (row, column) = window
            .cursor_place(&display, None)
            .map_or((0, 0), |(row, origin)| (row, column.saturating_sub(origin)));
        let cell = match side {
            Side::Left | Side::Right => text.row + row.min(text.rows.saturating_sub(1)),
            Side::Up | Side::Down => text.column + column.min(text.columns.saturating_sub(1)),
        };
        let other = self.frame.neighbour(id, side, cell).ok_or(NoWindow(side))?;
        self.select_window(other);
        Ok(())
    }

    /// Makes the selected tiled window `cells` rows taller with [`Way::Down`], or columns wider
    /// with [`Way::Across`], or smaller when negative, taking them from its neighbours that
    /// way, nearest first, as [`Splits::resize`] does. A window that can give or take only
    /// some of them moves that many. Zero does nothing.
    ///
    /// # Errors
    ///
    /// [`CannotResize`] when nothing moved. Nothing changes.
    pub fn resize_window(&mut self, way: Way, cells: isize) -> Result<(), CannotResize> {
        if cells == 0 {
            return Ok(());
        }
        let id = self.tiled_id();
        self.set_split_gutter();
        if self.splits.resize(id, way, cells, &self.frame) == 0 {
            return Err(if cells > 0 {
                CannotResize::Enlarge
            } else {
                CannotResize::Shrink
            });
        }
        self.lay_out(self.frame.size());
        Ok(())
    }

    /// Gives every child of every run of the tree the same weight, as `C-x +` does. Returns
    /// whether any window's cells changed.
    pub fn balance_windows(&mut self) -> bool {
        self.splits.balance();
        let before = self.frame.clone();
        self.lay_out(self.frame.size());
        self.frame != before
    }
}

#[cfg(test)]
mod buffer_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wheel_tests;
