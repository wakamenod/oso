//! Whole trees of tiled windows taken out of the frame and put back (ADR 0121): each space of
//! agent sessions keeps its own, and `space-zoom` swaps in a tree of one window and back.
//!
//! Must not depend on: `command`, `keymap` or the host's window. The host keeps the trees it
//! takes; this swaps them with the frame's.
//!
//! A tree moves whole: its windows keep their cursors, views, places and the pair they are in
//! (ADR 0122), and the tree keeps which window was selected last. The side window stays where
//! it is. What changed while a tree was out is settled when it comes back: a window whose
//! buffer was killed shows what a kill would have shown, places in killed buffers are
//! forgotten, and cursors are clipped to the text as it is now.

use std::fmt;

use super::{Editor, pairs::Pair};
use crate::buffer::BufferId;
use crate::frame::{Splits, Tiling};
use crate::window::{Window, WindowId};

use super::follow::clip_selections;

/// A tree of tiled windows out of the frame, for [`Editor::swap_window_tree`].
pub struct WindowTree {
    splits: Splits,
    /// Its windows, in the order they were made.
    windows: Vec<Window>,
    /// Its windows in the order they were selected, the most recent first.
    recent: Vec<WindowId>,
    pairs: Vec<Pair>,
}

impl fmt::Debug for WindowTree {
    /// Its windows' names in the tree's order, and no position: they may be in a buffer that
    /// holds secrets (ADR 0063).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowTree")
            .field("windows", &self.splits.iter().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl WindowTree {
    /// Its shape, with each child's weight, as [`Editor::tiling`] gives the frame's, for a
    /// layout to be saved.
    pub fn tiling(&self) -> Tiling {
        self.splits.tiling()
    }

    /// The buffer each of its windows shows, in the tree's order.
    pub fn buffers(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.splits.iter().filter_map(|id| {
            self.windows
                .iter()
                .find(|window| window.id() == id)
                .map(Window::buffer)
        })
    }
}

impl Editor {
    /// Puts `tree` in the frame in place of the tree there, which it gives back. The side
    /// window stays, and stays selected if it was; otherwise `tree`'s window selected last is
    /// selected. Ends isearch and a jump.
    ///
    /// # Errors
    ///
    /// `tree` itself, unchanged, while a prompt is open: what it acts on keeps its window.
    pub fn swap_window_tree(&mut self, tree: WindowTree) -> Result<WindowTree, WindowTree> {
        if !self.prompts.is_empty() {
            return Err(tree);
        }
        self.isearch_exit();
        self.replace_abandon();
        self.jump_end();
        self.window_moves_on();
        self.note_secret_shown();
        // The budget its wraps hold stays with the frame's windows.
        self.wrap_pool = self.gather_wrap_budget();
        let side = self.side_window();
        let side_selected = self.side_selected();
        let last = self.tree_id();
        let (out, side_window): (Vec<Window>, Vec<Window>) = std::mem::take(&mut self.windows)
            .into_iter()
            .partition(|window| Some(window.id()) != side);
        let mut recent: Vec<WindowId> = std::mem::take(&mut self.recent);
        recent.retain(|id| Some(*id) != side && *id != last);
        recent.insert(0, last);
        let old = WindowTree {
            splits: std::mem::replace(&mut self.splits, tree.splits),
            windows: out,
            recent,
            pairs: std::mem::take(&mut self.pairs),
        };
        self.windows = tree.windows;
        self.windows.extend(side_window);
        self.recent = tree.recent;
        self.pairs = tree.pairs;
        self.moved_windows.clear();
        if side_selected && let Some(side) = side {
            self.recent.insert(0, side);
        }
        let first = self.recent.first().copied();
        self.selected = first
            .and_then(|id| self.window_index(id))
            .unwrap_or_default();
        let back: Vec<usize> = (0..self.windows.len())
            .filter(|&index| Some(self.windows[index].id()) != side)
            .collect();
        // Every window shows a buffer that exists before anything lays the frame out.
        let moved: Vec<usize> = back
            .iter()
            .copied()
            .filter(|&index| self.settle_buffer(index))
            .collect();
        for index in moved {
            let buffer = self.windows[index].buffer();
            self.leave_pair(index, buffer);
        }
        for index in back {
            self.settle_view(index);
        }
        self.set_split_gutter();
        self.set_frame_size(self.frame.size());
        self.fit_gutter();
        self.note_secret_shown();
        Ok(old)
    }

    /// A tree of one or two windows for a space entered the first time (ADR 0121): `buffers`
    /// side by side, the first selected, each where the tiled window selected most recently
    /// that showed it is in it. No buffer, more than two, or one that does not exist, gives one
    /// window like the selected one ([`Editor::lone_window_tree`]).
    pub fn window_tree_showing(&mut self, buffers: &[BufferId]) -> WindowTree {
        let fresh = |editor: &mut Self, buffer: BufferId| {
            let mut window = editor.new_window(buffer);
            if let Some(place) = editor.place_in_other(None, buffer)
                && let Some(shown) = editor.buffer(buffer)
            {
                window.restore_positions(place.selections, place.scroll, &shown.display());
            }
            window
        };
        let exist = buffers.iter().all(|id| self.buffer(*id).is_some());
        match buffers {
            [one] if exist => {
                let window = fresh(self, *one);
                let id = window.id();
                WindowTree {
                    splits: Splits::new(id),
                    windows: vec![window],
                    recent: vec![id],
                    pairs: Vec::new(),
                }
            }
            [left, right] if exist => {
                let (left, right) = (fresh(self, *left), fresh(self, *right));
                let ids = [left.id(), right.id()];
                WindowTree {
                    splits: Splits::side_by_side(ids[0], ids[1]),
                    windows: vec![left, right],
                    recent: ids.to_vec(),
                    pairs: Vec::new(),
                }
            }
            _ => self.lone_window_tree(),
        }
    }

    /// A tree of one window like the tree's selected one, as a split makes it: the same buffer
    /// and view, and its primary cursor alone (ADR 0112). `space-zoom` swaps it in.
    pub fn lone_window_tree(&mut self) -> WindowTree {
        let at = self.tree_index();
        let id = WindowId::new(self.next_window);
        self.next_window += 1;
        let mut window = self.windows[at].split_off(id);
        if let Some(wrap) = window.wrap_map_mut() {
            wrap.take_budget();
        }
        WindowTree {
            splits: Splits::new(id),
            windows: vec![window],
            recent: vec![id],
            pairs: Vec::new(),
        }
    }

    /// Moves the window at `index`, just put back, off a buffer killed while it was out, as a
    /// kill moves a window: to the user's buffer it showed before, or `*scratch*`. Forgets its
    /// places in killed buffers. Returns whether it moved.
    fn settle_buffer(&mut self, index: usize) -> bool {
        let shown = self.windows[index].buffer();
        let dead = self.buffer(shown).is_none();
        let back = dead.then(|| {
            self.windows[index]
                .places()
                .map(|(id, _)| id)
                .find(|id| self.buffer(*id).is_some_and(|buffer| !buffer.is_internal()))
        });
        let back = back.map(|back| back.unwrap_or_else(|| self.scratch()));
        let place = back.and_then(|back| self.windows[index].show(back));
        let gone: Vec<BufferId> = self.windows[index]
            .places()
            .map(|(id, _)| id)
            .filter(|id| self.buffer(*id).is_none())
            .collect();
        let buffers = &self.buffers;
        let window = &mut self.windows[index];
        for id in gone {
            window.places_mut().forget(id);
        }
        window
            .way_back_mut()
            .retain(|id| buffers.iter().any(|(other, _)| other == id));
        if let Some(place) = place {
            window.selections = place.selections;
            window.set_scroll(place.scroll);
        }
        dead
    }

    /// Clips the cursors and the view of the window at `index`, just put back, to the text as
    /// it is now, and wraps it again where its buffer changed.
    fn settle_view(&mut self, index: usize) {
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let mut selections = std::mem::take(&mut window.selections);
        clip_selections(&mut selections, &display, &[], false, None);
        let scroll = window.scroll();
        window.restore_positions(selections, scroll, &display);
        self.sync_wrap(index);
    }
}

#[cfg(test)]
mod tests;
