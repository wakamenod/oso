//! `*Spaces*` in the side window (ADR 0121, Q1): shown on its own while oso has a session
//! that is not archived, until the person hides it; rendered to the side window's width only
//! while it is shown, with point kept on the row it was on.
//!
//! Must not depend on: how a frame is drawn, or key presses.
//!
//! `*Spaces*` is an internal buffer, read-only, and withheld from agents: its recaps are
//! sessions' screen text (Q7). While it is shown, the agent channel's commands answer in the
//! echo area only. Its marks and quiet text are drawn in the state faces, as highlights the
//! list gives with its text, and a working mark spins while one is in view (ADR 0131), and a
//! waiting row blinks, its faces alone set again on each beat (ADR 0140, as amended). Which
//! of its lines a section's band or the front worktree's tint is behind is kept for the frame,
//! and its modeline says how to enter it or leave it as it is selected or not (ADR 0140).

use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::selection::SelectionSet;
use oso_core::syntax::Highlights;

use crate::app::App;
use crate::sessions::list::{Entry, Key, Line, Rendered, SpaceBands};
use crate::sessions::recap::{clean, without_spinner};
use crate::sessions::spinner::Clock;
use crate::sessions::width;

/// The list's buffer name.
pub(in crate::app) const BUFFER: &str = "*Spaces*";

/// What the app keeps of `*Spaces*`.
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
    /// The text it was last given, so that the same text is not set again.
    text: String,
    /// The highlights it was last given, so that a blink's beat sets its faces alone.
    highlights: Vec<(std::ops::Range<usize>, oso_core::syntax::Highlight)>,
    /// The pane's clock: the spinner of working marks and the blink of waiting rows.
    pub(super) clock: Clock,
    /// oso's window is hidden: nothing spins.
    occluded: bool,
    /// The tree's projects and their listings (ADR 0131).
    pub(super) pane: super::pane::Pane,
    /// The pane's width as `<` and `>` left it, for this oso (ADR 0131, Q7).
    columns: Option<usize>,
    /// `[sessions] pane_width` in force.
    configured: Option<usize>,
    /// Its lines on a band, as last rendered, for the frame.
    bands: SpaceBands,
    /// How deep the tree showed open, as last rendered, for `S-TAB`.
    pub(super) open_to: u8,
    /// The keys of `sessions` its modeline names, as bound when last asked (ADR 0143), until
    /// the user's keys or menus change.
    enter_keys: Option<String>,
}

impl super::Watch {
    /// The lines of the list drawn on a band, as last rendered (ADR 0140): a method of the
    /// watch alone, so that a frame borrows it beside the app's view.
    pub(in crate::app) fn pane_bands(&self) -> &SpaceBands {
        &self.view.bands
    }

    /// `[sessions.notify]` in force.
    pub(in crate::app) fn set_toggles(&mut self, toggles: crate::sessions::attention::Toggles) {
        self.attention.set_toggles(toggles);
    }

    /// `[sessions] spinner` in force (ADR 0131, Q14).
    pub(in crate::app) fn set_spinner(&mut self, on: bool) {
        self.view.clock.set_spinner(on);
        self.stale = true;
    }

    /// `[sessions] blink` in force (ADR 0140, as amended).
    pub(in crate::app) fn set_blink(&mut self, on: bool) {
        self.view.clock.set_blink(on);
        self.stale = true;
    }
}

impl App {
    /// The pane's width in force (ADR 0131, Q7): as `<` and `>` left it, else
    /// `[sessions] pane_width`, else 44 columns.
    fn pane_columns(&self) -> usize {
        let view = &self.watch.view;
        view.columns.or(view.configured).unwrap_or(width::DEFAULT)
    }

    /// `[sessions] pane_width` in force: a new one replaces the width `<` and `>` left.
    pub(in crate::app) fn set_pane_width(&mut self, configured: Option<usize>) {
        if configured != self.watch.view.configured {
            self.watch.view.configured = configured;
            self.watch.view.columns = None;
            self.refit_sessions_side();
        }
    }

    /// `<` and `>`: the pane `columns` wider, narrower if negative, within 24 to 80, for this
    /// oso; never written to the configuration (the author, 2026-11-15).
    pub(super) fn change_pane_width(&mut self, columns: isize) {
        let from = self.pane_columns();
        let to = width::changed(from, columns);
        if to == from {
            let edge = if columns > 0 { "widest" } else { "narrowest" };
            return self
                .editor
                .message(format!("*Spaces* is at its {edge}, {to} columns"));
        }
        self.watch.view.columns = Some(to);
        self.refit_sessions_side();
        self.editor.message(format!("*Spaces*: {to} columns"));
    }

    /// The side window at the width in force while it shows `*Spaces*`, rendered again.
    fn refit_sessions_side(&mut self) {
        if self.sessions_list_shown()
            && let Some(buffer) = self.watch.view.buffer
        {
            self.editor.show_side_window(buffer, self.pane_columns());
        }
        self.watch.stale = true;
    }

    /// The `*Spaces*` buffer: the one kept, or a new one, withheld from agents.
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

    /// Whether the side window shows `*Spaces*`.
    pub(super) fn sessions_list_shown(&self) -> bool {
        let Some(side) = self.editor.side_window() else {
            return false;
        };
        self.watch.view.buffer.is_some()
            && self.editor.window_by_id(side).map(|w| w.buffer()) == self.watch.view.buffer
    }

    /// Shows `*Spaces*` in the side window, not selected, rendered.
    pub(super) fn show_sessions_side(&mut self) {
        let buffer = self.sessions_buffer();
        self.editor.show_side_window(buffer, self.pane_columns());
        self.watch.view.shown = true;
        self.watch.view.hidden = false;
        self.watch.stale = true;
    }

    /// Shows the side window on its own while a session is not archived, unless the person
    /// hid it (Q1), and renders `*Spaces*` while it is shown. Returns whether what shows
    /// changed.
    pub(super) fn show_sessions_list(&mut self, entries: &[Entry], now: Instant) -> bool {
        let mut changed = false;
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
            self.editor.show_side_window(buffer, self.pane_columns());
            self.watch.view.shown = true;
            self.watch.view.text.clear();
            changed = true;
        }
        if self.sessions_list_shown() {
            changed |= self.render_sessions(entries, now);
        } else {
            self.watch.view.clock.stop();
        }
        changed
    }

    /// Whether `*Spaces*` could blink now: shown, not hidden by the system, with `[sessions]
    /// blink` on.
    pub(super) fn sessions_may_blink(&self) -> bool {
        !self.watch.view.occluded && self.watch.view.clock.blinks() && self.sessions_list_shown()
    }

    /// When the spinner's next frame or the blink's next beat is due, while it runs.
    pub(super) fn sessions_clock_due(&self) -> Option<Instant> {
        self.watch.view.clock.due()
    }

    /// The spinner's next frame or the blink's next beat, if due at `now`: the list is drawn
    /// again with it, as its ages are, so that no candidate is made again.
    pub(super) fn tick_sessions(&mut self, now: Instant) {
        if self.watch.view.clock.due().is_some_and(|due| now >= due) {
            self.watch.stale = true;
            self.watch.aging = true;
        }
    }

    /// oso's window was hidden, or shown again: the spinner stops while it is hidden, and
    /// starts again with the next drawing of the list.
    pub(in crate::app) fn sessions_occluded(&mut self, occluded: bool) {
        self.watch.view.occluded = occluded;
        if occluded {
            self.watch.view.clock.stop();
        } else {
            self.watch.stale = true;
        }
    }

    /// Renders `*Spaces*` to the side window's width, point kept on what its row showed.
    /// Returns whether what it shows changed: its text, its faces or its lit cells.
    fn render_sessions(&mut self, entries: &[Entry], now: Instant) -> bool {
        let (Some(side), Some(buffer)) = (self.editor.side_window(), self.watch.view.buffer) else {
            return false;
        };
        let tree = self.sessions_tree();
        self.watch.list.set_tree(tree);
        self.note_pane_focus();
        let Some(window) = self.editor.window_by_id(side) else {
            return false;
        };
        let width = window.size().columns;
        let cursor = window.selections.primary().cursor;
        let scroll = window.scroll();
        let on = self
            .editor
            .buffer(buffer)
            .map(|text| text.text().offset_to_point(cursor).row);
        let was = on.and_then(|line| self.watch.view.lines.get(line).cloned());
        let shown = !self.watch.view.occluded;
        let frame = shown.then(|| self.watch.view.clock.frame(now)).flatten();
        self.watch.list.set_spin(frame);
        let lit = shown.then(|| self.watch.view.clock.lit(now)).flatten();
        let watched = self.watched_session().map(Key::Hosted);
        self.watch.list.set_blink(lit, watched);
        let mut rendered = self.watch.list.render(entries, width, now);
        let top = scroll.start;
        let rows = window.size().rows;
        let first = self.editor.buffer(buffer).map_or(0, |text| {
            text.text().offset_to_point(top.min(text.text().len())).row
        });
        let in_view = |lines: &[usize]| {
            lines
                .iter()
                .any(|line| (first..first + rows).contains(line))
        };
        let spinning = frame.is_some() && in_view(&rendered.spinning);
        let blinking = lit.is_some() && in_view(&rendered.blinking);
        self.watch.view.clock.drawn(spinning, blinking, now);
        let same = rendered.text == self.watch.view.text
            && self
                .editor
                .buffer(buffer)
                .is_some_and(|kept| kept.text().len() == rendered.text.len());
        let lit_moved = rendered.bands.lit != self.watch.view.bands.lit;
        self.watch.view.bands = std::mem::take(&mut rendered.bands);
        self.watch.view.open_to = rendered.open_to;
        let same_faces = same && rendered.highlights == self.watch.view.highlights;
        if !same_faces {
            // A blink's beat changes faces alone: its text is not set again, so nothing is
            // shaped again (ADR 0140, as amended).
            if !same {
                self.editor.set_internal_text(buffer, &rendered.text);
            }
            self.editor
                .set_internal_highlights(buffer, highlights_of(&rendered));
            self.watch.view.highlights = std::mem::take(&mut rendered.highlights);
        }
        if same {
            self.watch.view.lines = rendered.lines;
            return !same_faces || lit_moved;
        }
        let line = was
            .and_then(|was| rendered.lines.iter().position(|line| *line == was))
            .or(on.filter(|&line| line < rendered.lines.len()))
            // Not on the title, where no key acts: on the first row under it (ADR 0140).
            .map(|line| match rendered.lines.get(line) {
                Some(Line::Title) if rendered.lines.len() > 1 => line + 1,
                _ => line,
            });
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
        self.watch.view.text = rendered.text;
        true
    }

    /// Session `session`'s terminal's title as its row may show it (ADR 0140, Q2): cleaned as a
    /// recap is, without its spinner, and with no braille blank, which draws as nothing and so
    /// could pad a label. Empty while it has no terminal.
    pub(super) fn row_title(&self, session: &crate::sessions::hosted::Hosted) -> String {
        (session.buffer)
            .and_then(|id| self.terminals.map.get(id))
            .map(|terminal| {
                let title = without_spinner(&terminal.title).replace('\u{2800}', " ");
                clean(&title)
            })
            .unwrap_or_default()
    }

    /// Says on the pane's modeline how to enter it, or, while it is selected, how to leave it
    /// (ADR 0140): before each frame, as the selection may have changed with any key.
    pub(in crate::app) fn note_pane_focus(&mut self) {
        let (Some(side), Some(buffer)) = (self.editor.side_window(), self.watch.view.buffer) else {
            return;
        };
        let selected = self.editor.tiled().0.id() == side;
        // From any buffer, terminals among them: the global keys and the user's.
        let dispatcher = &self.dispatcher;
        let enter = (self.watch.view.enter_keys)
            .get_or_insert_with(|| dispatcher.where_is_anywhere("sessions"));
        let note = self.watch.list.modeline_note(selected, enter);
        if (self.editor.buffer(buffer)).is_some_and(|b| b.mode_note() != note) {
            self.editor.set_mode_note(buffer, note);
        }
    }

    /// Forgets the keys the pane's modeline names, as the user's keys or menus changed.
    pub(in crate::app) fn forget_pane_keys(&mut self) {
        self.watch.view.enter_keys = None;
    }

    /// What line `line` of `*Spaces*` shows, as last rendered.
    pub(super) fn sessions_line(&self, line: usize) -> Option<&Line> {
        self.watch.view.lines.get(line)
    }
}

/// The highlights of the list's text, as it gives them (ADR 0131).
fn highlights_of(rendered: &Rendered) -> Highlights {
    let mut highlights = Highlights::new(rendered.text.len());
    for (range, highlight) in &rendered.highlights {
        highlights.replace(range.clone(), [(range.len(), Some(*highlight))]);
    }
    highlights
}
