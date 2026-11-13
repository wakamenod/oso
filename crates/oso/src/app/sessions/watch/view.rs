//! `*Sessions*` in the side window (ADR 0121, Q1): shown on its own while oso has a session
//! that is not archived, until the person hides it; rendered to the side window's width only
//! while it is shown, with point kept on the row it was on.
//!
//! Must not depend on: how a frame is drawn, or key presses.
//!
//! `*Sessions*` is an internal buffer, read-only, and withheld from agents: its recaps are
//! sessions' screen text (Q7). While it is shown, the agent channel's commands answer in the
//! echo area only.

use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::selection::SelectionSet;

use crate::app::App;
use crate::sessions::list::{Entry, Key, Line};

/// The side window's width, in columns (Q1).
pub(in crate::app) const SIDE_COLUMNS: usize = 32;

/// The list's buffer name.
pub(in crate::app) const BUFFER: &str = "*Sessions*";

/// What the app keeps of `*Sessions*`.
#[derive(Debug, Default)]
pub(super) struct View {
    /// The internal buffer, once made. Kept by its id, never found by its name.
    pub(super) buffer: Option<BufferId>,
    /// What each line of it shows, as last rendered.
    pub(super) lines: Vec<Line>,
    /// oso showed it in the side window, and has not seen it go since.
    pub(super) shown: bool,
    /// The person hid it: it is not shown again on its own while this oso runs.
    pub(super) hidden: bool,
}

impl App {
    /// The `*Sessions*` buffer: the one kept, or a new one, withheld from agents.
    pub(super) fn sessions_buffer(&mut self) -> BufferId {
        let kept = self
            .watch
            .view
            .buffer
            .filter(|id| self.editor.buffer(*id).is_some_and(Buffer::is_internal));
        let id = kept.unwrap_or_else(|| self.editor.add_internal_buffer(BUFFER, Mode::Sessions));
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold_from_agents();
        }
        self.watch.view.buffer = Some(id);
        id
    }

    /// Whether the side window shows `*Sessions*`.
    pub(super) fn sessions_list_shown(&self) -> bool {
        let Some(side) = self.editor.side_window() else {
            return false;
        };
        self.watch.view.buffer.is_some()
            && self.editor.window_by_id(side).map(|w| w.buffer()) == self.watch.view.buffer
    }

    /// Shows `*Sessions*` in the side window, not selected, rendered.
    pub(super) fn show_sessions_side(&mut self) {
        let buffer = self.sessions_buffer();
        self.editor.show_side_window(buffer, SIDE_COLUMNS);
        self.watch.view.shown = true;
        self.watch.view.hidden = false;
        self.watch.stale = true;
    }

    /// Takes `*Sessions*` off the frame: the person hid it.
    pub(super) fn hide_sessions_side(&mut self) {
        if self.sessions_list_shown() {
            self.editor.hide_side_window();
        }
        self.watch.view.shown = false;
        self.watch.view.hidden = true;
    }

    /// Shows the side window on its own while a session is not archived, unless the person
    /// hid it (Q1), and renders `*Sessions*` while it is shown.
    pub(super) fn show_sessions_list(&mut self, entries: &[Entry], now: Instant) {
        if self.watch.view.shown && !self.sessions_list_shown() {
            // `C-x 0` in it, or another command, took it away.
            self.watch.view.shown = false;
            self.watch.view.hidden = true;
        }
        let any = entries
            .iter()
            .any(|entry| matches!(entry.key, Key::Hosted(_)) && !entry.archived);
        if any && !self.watch.view.hidden && self.editor.side_window().is_none() {
            let buffer = self.sessions_buffer();
            self.editor.show_side_window(buffer, SIDE_COLUMNS);
            self.watch.view.shown = true;
        }
        if self.sessions_list_shown() {
            self.render_sessions(entries, now);
        }
    }

    /// Renders `*Sessions*` to the side window's width, point kept on what its row showed.
    fn render_sessions(&mut self, entries: &[Entry], now: Instant) {
        let (Some(side), Some(buffer)) = (self.editor.side_window(), self.watch.view.buffer) else {
            return;
        };
        let Some(window) = self.editor.window_by_id(side) else {
            return;
        };
        let width = window.size().columns;
        let cursor = window.selections.primary().cursor;
        let scroll = window.scroll();
        let on = self
            .editor
            .buffer(buffer)
            .map(|text| text.text().offset_to_point(cursor).row);
        let was = on.and_then(|line| self.watch.view.lines.get(line).cloned());
        let rendered = self.watch.list.render(entries, width, now);
        self.editor.set_internal_text(buffer, &rendered.text);
        let line = was
            .and_then(|was| rendered.lines.iter().position(|line| *line == was))
            .or(on.filter(|&line| line < rendered.lines.len()));
        if let Some(line) = line {
            let offset: usize = rendered
                .text
                .split_inclusive('\n')
                .take(line)
                .map(str::len)
                .sum();
            self.editor
                .restore_positions_in(side, SelectionSet::new(offset), scroll);
        }
        self.watch.view.lines = rendered.lines;
    }

    /// What line `line` of `*Sessions*` shows, as last rendered.
    pub(super) fn sessions_line(&self, line: usize) -> Option<&Line> {
        self.watch.view.lines.get(line)
    }
}
