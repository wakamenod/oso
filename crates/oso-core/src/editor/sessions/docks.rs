//! Docked windows (ADR 0170): windows the split tree does not hold, laid out inside the tiled
//! window that shows their host buffer, from the rows it gives up at its bottom, above its
//! modeline. A native session's buffer hosts its to-do strip, its permission card and its
//! composer this way.
//!
//! Must not depend on: the host's threads or the window system; plain state over the editor's
//! windows.
//!
//! - A docked window is one of [`Editor::windows`]'s tiled windows, marked as docked under its
//!   host buffer and slot; the tree never holds it.
//! - One window carries a host's docks: the one showing the host that was selected last, else
//!   the first in the frame's order. None showing it hides them; they keep their state.
//! - The composer is as many rows as its text wraps to, or its lines while it truncates them, 1
//!   to [`MAX_COMPOSER`], and the strip at most [`MAX_STRIP`]; the card as many as its text has
//!   lines, at most [`MAX_CARD`]: its host breaks its rows at its width, and it never wraps
//!   (ADR 0180). The composer's rows are given first, the card's next. A request's line, in the
//!   card's slot, gives up none of its rows, as its keys are all shown (ADR 0182).
//! - A composer has a row above and below its text, where there is room, which the host draws its
//!   rules in; and a host whose buffer has a footer ([`Editor::set_footer`]) a row under its docks.
//! - A host no window of the tree shows, as a docked buffer is, hides what is docked under it.

use super::super::Editor;
use crate::buffer::{BufferId, Mode};
use crate::display_map::wrap_map;
use crate::frame::docks::DockAsk;
use crate::window::{DockSlot, Window, WindowId};

/// The most rows the composer takes; past them it scrolls: as many as a review's draft takes
/// (ADR 0122).
pub(in crate::editor) const MAX_COMPOSER: usize = crate::minibuffer::MAX_DRAFT_ROWS;
/// The most rows the card takes: a question's or a plan's grows to what it says, as its host
/// draws it (ADR 0180).
pub(in crate::editor) const MAX_CARD: usize = 60;
/// The most rows the strip takes.
pub(in crate::editor) const MAX_STRIP: usize = 6;

/// What the editor keeps of the docks between layouts.
#[derive(Debug, Default)]
pub(in crate::editor) struct Docks {
    /// Each docked window laid out last, with the window it is laid out in.
    carried: Vec<(WindowId, WindowId)>,
    /// The rows each docked window wanted at the last layout.
    wanted: Vec<(WindowId, usize)>,
}

impl Editor {
    /// Docks `buffer` under `host`'s window in `slot`, or, with `None`, takes the docked window
    /// there away (ADR 0170). A slot that shows another buffer shows `buffer` in the same window,
    /// which keeps its place. The frame is laid out again.
    ///
    /// Nothing changes for a host or a buffer that does not exist, a host docked under itself,
    /// or a buffer docked in another slot already.
    pub fn set_dock(&mut self, host: BufferId, slot: DockSlot, buffer: Option<BufferId>) {
        let docked_elsewhere = |editor: &Self, buffer: BufferId| {
            editor.windows[..editor.tiled_count()].iter().any(|window| {
                window.buffer() == buffer
                    && window.docked().is_some_and(|docked| docked != (host, slot))
            })
        };
        if self.buffer(host).is_none()
            || buffer.is_some_and(|buffer| {
                buffer == host || self.buffer(buffer).is_none() || docked_elsewhere(self, buffer)
            })
        {
            return;
        }
        let at = self.dock_index(host, slot);
        match (at, buffer) {
            (Some(at), Some(buffer)) if self.windows[at].buffer() != buffer => {
                self.window_shows(at, buffer);
            }
            (Some(_), Some(_)) | (None, None) => return,
            (None, Some(buffer)) => {
                let id = WindowId::new(self.next_window);
                self.next_window += 1;
                self.insert_outside_tree(Window::new_docked(id, buffer, host, slot));
            }
            (Some(at), None) => {
                let id = self.windows[at].id();
                if self.tiled_id() == id {
                    self.leave_side();
                }
                self.drop_window(id);
            }
        }
        self.lay_out(self.frame.size());
    }

    /// The window docked under `host` in `slot`, if there is one.
    pub fn dock_window(&self, host: BufferId, slot: DockSlot) -> Option<WindowId> {
        self.dock_index(host, slot).map(|at| self.windows[at].id())
    }

    fn dock_index(&self, host: BufferId, slot: DockSlot) -> Option<usize> {
        self.windows[..self.tiled_count()]
            .iter()
            .position(|window| window.docked() == Some((host, slot)))
    }

    /// Whether `buffer` is shown in a docked window: its host's, never listed or raised as a
    /// buffer of its own (ADR 0170).
    pub(in crate::editor) fn docked_buffer(&self, buffer: BufferId) -> bool {
        self.windows[..self.tiled_count()]
            .iter()
            .any(|window| window.docked().is_some() && window.buffer() == buffer)
    }

    /// Whether `id` is a docked window.
    pub(in crate::editor) fn is_docked(&self, id: WindowId) -> bool {
        self.window_by_id(id)
            .is_some_and(|window| window.docked().is_some())
    }

    /// Whether the selected tiled window, or the one beneath the prompts, is a docked one.
    pub(in crate::editor) fn dock_selected(&self) -> bool {
        self.windows[self.tiled_index()].docked().is_some()
    }

    /// The window docked window `id` was laid out in last, if it was.
    pub(in crate::editor) fn dock_host(&self, id: WindowId) -> Option<WindowId> {
        (self.sessions.docks.carried.iter())
            .find_map(|&(docked, host)| (docked == id).then_some(host))
    }

    /// Lays out every host's docks inside the window that carries them, once the tree's windows
    /// have their regions: each docked window gets regions of its own, and its host's text the
    /// rows left.
    pub(in crate::editor) fn lay_out_docks(&mut self) {
        self.sessions.docks.carried.clear();
        self.sessions.docks.wanted.clear();
        let count = self.tiled_count();
        let mut hosts: Vec<BufferId> = Vec::new();
        for window in &self.windows[..count] {
            if let Some((host, _)) = window.docked()
                && !hosts.contains(&host)
            {
                hosts.push(host);
            }
        }
        for host in hosts {
            let Some(carrier) = self.carrier(host) else {
                continue;
            };
            let Some(area) = self.frame.regions(carrier).map(|regions| regions.area()) else {
                continue;
            };
            let asks: Vec<DockAsk> = DockSlot::ALL
                .iter()
                .filter_map(|&slot| self.dock_index(host, slot).map(|at| (slot, at)))
                .map(|(slot, at)| self.dock_ask(slot, at, area.columns))
                .collect();
            self.sessions
                .docks
                .wanted
                .extend(asks.iter().map(|ask| (ask.window, ask.rows)));
            let footer = (self.buffer(host)).is_some_and(|buffer| buffer.beside_footer().is_some());
            if self.frame.dock(carrier, &asks, footer) {
                let carried = asks.iter().map(|ask| (ask.window, carrier));
                self.sessions.docks.carried.extend(carried);
            }
        }
    }

    /// The window that carries `host`'s docks: the tree's window showing it that was selected
    /// last, else the first in the frame's order, if the frame has room for it.
    fn carrier(&self, host: BufferId) -> Option<WindowId> {
        let shows = |id: &WindowId| {
            self.splits.contains(*id)
                && self
                    .window_by_id(*id)
                    .is_some_and(|window| window.buffer() == host)
                && (self.frame.regions(*id)).is_some_and(crate::frame::WindowRegions::has_cells)
        };
        let order = self.frame.windows().iter().map(|(id, _)| id);
        self.recent
            .iter()
            .chain(order)
            .copied()
            .find(|id| shows(id))
    }

    /// What the docked window at `at`, in `slot`, asks of a host `columns` wide.
    fn dock_ask(&self, slot: DockSlot, at: usize, columns: usize) -> DockAsk {
        let window = &self.windows[at];
        let buffer = self.shown(window);
        let (most, least, rank) = match slot {
            DockSlot::Composer => (MAX_COMPOSER, 1, 0),
            DockSlot::Card => (MAX_CARD, 0, 1),
            DockSlot::Strip => (MAX_STRIP, 0, 2),
        };
        let gutter = super::super::gutter::dock_gutter(slot);
        let width = columns.saturating_sub(gutter).max(1);
        let rows = match slot {
            // A row a line: its host breaks its rows at its width, and fills them (ADR 0180).
            DockSlot::Card => buffer.text().line_count().min(most),
            // As its window shows them: a person may truncate its lines (ADR 0165).
            DockSlot::Composer if !buffer.wraps() => buffer.text().line_count().min(most),
            _ => wrap_map::rows_at_most(&buffer.display(), width, most),
        };
        let rows = rows.max(least);
        // Its keys are all shown, or no dock is (ADR 0182).
        let least = match buffer.mode() {
            Mode::Request => rows,
            _ => least,
        };
        DockAsk {
            window: window.id(),
            rows,
            least,
            rank,
            gutter,
            padded: slot == DockSlot::Composer && buffer.mode() == Mode::Composer,
        }
    }

    /// What `host`'s modeline says in place of its mode's note in the window that shows its footer
    /// (ADR 0170): the host draws a footer, in the row [`FrameLayout::footer`] gives, under what is
    /// docked under `host`. With `None` it has no footer. The frame is laid out again as a footer
    /// comes or goes.
    ///
    /// [`FrameLayout::footer`]: crate::frame::FrameLayout::footer
    pub fn set_footer(&mut self, host: BufferId, note: Option<String>) {
        let has = note.is_some();
        let Some(buffer) = self.buffer_mut(host) else {
            return;
        };
        let had = buffer.beside_footer().is_some();
        buffer.set_beside_footer(note);
        if had != has {
            self.lay_out(self.frame.size());
        }
    }

    /// Lays the frame out again once a docked window's text changed what it asks: the composer
    /// grows and shrinks with its text.
    pub(in crate::editor) fn dock_edited(&mut self, buffer: BufferId) {
        let count = self.tiled_count();
        let Some(changed) = self.windows[..count]
            .iter()
            .position(|window| window.docked().is_some() && window.buffer() == buffer)
        else {
            return;
        };
        let id = self.windows[changed].id();
        let Some(&(_, wanted)) =
            (self.sessions.docks.wanted.iter()).find(|(docked, _)| *docked == id)
        else {
            return;
        };
        let (slot, columns) = match (self.windows[changed].docked(), self.dock_host(id)) {
            (Some((_, slot)), Some(host)) => {
                let columns = self
                    .frame
                    .regions(host)
                    .map_or(0, |regions| regions.area().columns);
                (slot, columns)
            }
            _ => return,
        };
        if self.dock_ask(slot, changed, columns).rows != wanted {
            self.lay_out(self.frame.size());
        }
    }

    /// Whether window `id`, just selected, shows a host whose docks are laid out in another
    /// window: they come to it.
    pub(in crate::editor) fn docks_elsewhere(&self, id: WindowId) -> bool {
        let Some(shown) = self.window_by_id(id).map(Window::buffer) else {
            return false;
        };
        self.windows[..self.tiled_count()].iter().any(|window| {
            window.docked().is_some_and(|(host, _)| host == shown)
                && self
                    .dock_host(window.id())
                    .is_some_and(|carrier| carrier != id)
        })
    }

    /// A window went from showing `before` to `after`: the frame is laid out again if either
    /// hosts docks, which go to the window that carries them now.
    pub(in crate::editor) fn docks_follow(&mut self, before: BufferId, after: BufferId) {
        let hosts = self.windows[..self.tiled_count()].iter().any(|window| {
            window
                .docked()
                .is_some_and(|(host, _)| host == before || host == after)
        });
        if hosts && before != after {
            self.lay_out(self.frame.size());
        }
    }

    /// The buffer `id` goes: a host's docks go with it, and a docked window showing it too.
    pub(in crate::editor) fn forget_docks_of(&mut self, id: BufferId) {
        let gone: Vec<WindowId> = self.windows[..self.tiled_count()]
            .iter()
            .filter(|window| {
                window
                    .docked()
                    .is_some_and(|(host, _)| host == id || window.buffer() == id)
            })
            .map(Window::id)
            .collect();
        for docked in gone {
            if self.tiled_id() == docked {
                self.leave_side();
            }
            self.drop_window(docked);
        }
    }

    /// Whether `window` hides its docks, having too few rows for them, for its modeline.
    pub(crate) fn docks_hidden(&self, window: WindowId) -> bool {
        self.frame.docks_hidden().contains(&window)
    }
}

#[cfg(test)]
mod tests;
