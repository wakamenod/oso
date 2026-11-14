//! The side window (ADR 0121): one window at the frame's left, outside the tree of tiled
//! windows, as Emacs's side windows are. Agent sessions' list shows in it.
//!
//! Must not depend on: `command`, `keymap` or the host's window. The frame's module lays it
//! out; this keeps which window it is and what the tree's commands do while it is selected.
//!
//! It is a tiled window, in [`Editor::windows`] before the prompts', but not in the tree, so
//! [`Editor::tiling`], a space's tree and `C-x +` never see it. `C-x o` skips it; a click and
//! `windmove-left` reach it. While it is selected, the tree's commands act on the tree's window
//! selected last, and select it: a split, a resize, `C-x 1` and showing a buffer, so that what
//! a command shows never lands in the side window. `C-x 0` in it hides it.

use crate::buffer::BufferId;
use crate::editor::Editor;
use crate::frame::SideColumns;
use crate::window::WindowId;

/// The side window: which window it is, and how wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct SideWindow {
    id: WindowId,
    columns: usize,
}

impl Editor {
    /// Shows `buffer` in the side window, `columns` wide beside a divider, making it first if
    /// there is none. It is not selected: [`Editor::select_window`] selects it. Returns its
    /// name. A buffer that does not exist changes nothing, and gives the side window's name,
    /// or the selected tiled window's while there is none.
    pub fn show_side_window(&mut self, buffer: BufferId, columns: usize) -> WindowId {
        if self.buffer(buffer).is_none() {
            return self.side_window().unwrap_or_else(|| self.tiled_id());
        }
        let id = match self.sessions.side {
            Some(side) => {
                if let Some(index) = self.window_index(side.id)
                    && self.windows[index].buffer() != buffer
                {
                    self.window_moves_on();
                    self.window_shows(index, buffer);
                }
                side.id
            }
            None => {
                let window = self.new_window(buffer);
                let id = window.id();
                let at = self.tiled_count();
                self.windows.insert(at, window);
                if self.selected >= at {
                    self.selected += 1;
                }
                self.sync_wrap(at);
                self.fit_gutter();
                self.note_secret_shown();
                id
            }
        };
        self.sessions.side = Some(SideWindow { id, columns });
        self.lay_out(self.frame.size());
        id
    }

    /// Takes the side window off the frame, selecting the tree's window selected last if it
    /// was selected. Returns whether there was one.
    pub fn hide_side_window(&mut self) -> bool {
        let Some(side) = self.sessions.side.take() else {
            return false;
        };
        self.isearch_exit();
        self.drop_window(side.id);
        self.lay_out(self.frame.size());
        true
    }

    /// The side window's name, while it is shown.
    pub fn side_window(&self) -> Option<WindowId> {
        self.sessions.side.map(|side| side.id)
    }

    /// What the frame lays out at its left.
    pub(in crate::editor) fn side_columns(&self) -> Option<SideColumns> {
        self.sessions.side.map(|side| SideColumns {
            window: side.id,
            columns: side.columns,
        })
    }

    /// Whether `id` is a tiled window: one of the tree's, or the side window.
    pub(in crate::editor) fn is_tiled_id(&self, id: WindowId) -> bool {
        self.splits.contains(id) || self.side_window() == Some(id)
    }

    /// Whether the side window is the selected tiled window, or the one beneath the prompts.
    pub(in crate::editor) fn side_selected(&self) -> bool {
        self.side_window().is_some_and(|id| id == self.tiled_id())
    }

    /// The tree's window the tree's commands act on: the selected tiled window, or, while the
    /// side window is, the tree's window selected last.
    pub(in crate::editor) fn tree_id(&self) -> WindowId {
        if !self.side_selected() {
            return self.tiled_id();
        }
        let last = self.recent.iter().find(|id| self.splits.contains(**id));
        last.copied()
            .or_else(|| self.splits.iter().next())
            .unwrap_or_else(|| self.tiled_id())
    }

    /// The position of [`Editor::tree_id`]'s window in [`Editor::windows`].
    pub(in crate::editor) fn tree_index(&self) -> usize {
        self.window_index(self.tree_id())
            .unwrap_or_else(|| self.tiled_index())
    }

    /// Selects the tree's window selected last while the side window is selected, as the
    /// tree's commands do before they act.
    pub(in crate::editor) fn leave_side(&mut self) {
        if self.side_selected() {
            self.make_tiled(self.tree_id());
        }
    }

    /// The position of the tree's window [`Editor::leave_side`] selects, which it then selects.
    pub(in crate::editor) fn leave_side_at(&mut self) -> usize {
        let index = self.tree_index();
        self.leave_side();
        index
    }
}

#[cfg(test)]
mod tests;
