//! A native session's to-do strip in the app (ADR 0165, part 3e; ADR 0170's `Strip` dock):
//! `*todo: NAME*`, docked between the session's transcript and its composer, under its card,
//! while the session's list has an item. Its text is `sessions::native::todo`'s, folded to one
//! line or opened by `C-c C-t` to at most six rows; this docks it at its window's width.
//!
//! Must not depend on: key presses or how a frame is drawn; the stream's events give the list,
//! and `native-session-todo` asks to open or fold it.
//!
//! - The buffer is internal, read-only and withheld from agents, as the card is: what it shows
//!   is what the session's calls say.
//! - It is drawn again when its list changes, when it opens or folds, and when its window's
//!   width changes, as its rows are cut at it; text the same as drawn last is not written
//!   again, and a frame is asked for only while a window shows the session.
//! - A resumed session's list is empty until its next to-do call (the author's Q-E2 (a)); a
//!   tier-1 interrupt's resume keeps it, as the turn goes on, if in a new process.

use oso_core::buffer::{BufferId, Mode};
use oso_core::syntax::Highlights;
use oso_core::window::DockSlot;

use crate::app::App;
use crate::sessions::native::todo::{Strip, Todo};
use crate::sessions::worker::SessionKey;

/// The width a strip is drawn at when no window shows it yet, in cells.
const UNSHOWN_WIDTH: usize = 80;

/// A native session's to-do list and its strip.
#[derive(Debug, Default)]
pub(super) struct TodoStrip {
    pub(super) list: Todo,
    /// Its `*todo: NAME*`, once its list had an item.
    pub(super) buffer: Option<BufferId>,
    /// `C-c C-t` opened it.
    open: bool,
    /// The width it was drawn at, while it is docked, and what it showed.
    drawn: Option<(usize, Strip)>,
    /// A tier-1 interrupt resumes the session: its list stays.
    keep: bool,
}

impl App {
    /// Session `key`'s strip drawn as its list is now: docked while the list has an item, and
    /// taken away once it has none, point going to the composer if it was there.
    pub(super) fn draw_todo(&mut self, key: SessionKey) {
        let width = self.todo_width(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let (transcript, strip) = (native.buffer, &mut native.todo);
        let shown = (self.editor.windows()).any(|window| window.buffer() == transcript);
        if !strip.list.shows() {
            strip.open = false;
            strip.drawn = None;
            let Some(buffer) = strip.buffer else {
                return;
            };
            let was_in = self.editor.window().buffer() == buffer;
            self.editor.set_dock(transcript, DockSlot::Strip, None);
            self.editor.set_internal_text(buffer, "");
            if was_in {
                self.composer_go(key, true);
            }
            if shown {
                self.request_redraw();
            }
            return;
        }
        let text = match strip.open {
            true => strip.list.opened(width),
            false => strip.list.folded(width),
        };
        let same = (strip.drawn.as_ref()).is_some_and(|drawn| *drawn == (width, text.clone()));
        let kept = strip.buffer.filter(|&id| self.editor.buffer(id).is_some());
        if same && kept.is_some() {
            return;
        }
        let Some(buffer) = self.todo_buffer(key) else {
            return;
        };
        self.write_todo(buffer, &text);
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.todo.drawn = Some((width, text));
        }
        self.editor
            .set_dock(transcript, DockSlot::Strip, Some(buffer));
        if shown {
            self.request_redraw();
        }
    }

    /// After each sync: each strip docked whose window's width changed is drawn at the new
    /// width.
    pub(in crate::app) fn sync_todo_strips(&mut self) {
        let resized: Vec<SessionKey> = (self.hosting.natives.map.iter())
            .filter_map(|(key, native)| Some((*key, native.todo.drawn.as_ref()?.0)))
            .filter(|&(key, width)| self.todo_width(key) != width)
            .map(|(key, _)| key)
            .collect();
        for key in resized {
            self.draw_todo(key);
        }
    }

    /// `C-c C-t` in session `key`'s transcript or composer: its strip opens, or folds back.
    pub(super) fn toggle_todo(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if !native.todo.list.shows() {
            return self.editor.message("This session has no to-do list");
        }
        native.todo.open = !native.todo.open;
        self.draw_todo(key);
    }

    /// Session `key` resumes: its list is empty until its next to-do call (Q-E2 (a)), unless
    /// a tier-1 interrupt resumes it ([`App::keep_todo`]).
    pub(super) fn clear_todo(&mut self, key: SessionKey) {
        let cleared = (self.hosting.natives.map.get_mut(&key))
            .is_some_and(|native| !native.todo.keep && native.todo.list.clear());
        if cleared {
            self.draw_todo(key);
        }
    }

    /// Whether session `key`'s list stays across the resume that follows, as a tier-1
    /// interrupt's does: the turn goes on, if in a new process.
    pub(super) fn keep_todo(&mut self, key: SessionKey, keep: bool) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.todo.keep = keep;
        }
    }

    /// The width session `key`'s strip is drawn at: its window's, else its transcript's
    /// whole, as the strip has no gutter (ADR 0170).
    fn todo_width(&self, key: SessionKey) -> usize {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return UNSHOWN_WIDTH;
        };
        if let Some(docked) = self.editor.dock_window(native.buffer, DockSlot::Strip) {
            return (self.editor.regions(docked)).map_or(UNSHOWN_WIDTH, |r| r.text.columns);
        }
        (self.editor.windows())
            .find(|window| window.buffer() == native.buffer)
            .and_then(|window| self.editor.regions(window.id()))
            .map_or(UNSHOWN_WIDTH, |regions| regions.area().columns)
    }

    /// Session `key`'s strip's buffer, made the first time.
    fn todo_buffer(&mut self, key: SessionKey) -> Option<BufferId> {
        let native = self.hosting.natives.map.get(&key)?;
        if let Some(id) = native
            .todo
            .buffer
            .filter(|&id| self.editor.buffer(id).is_some())
        {
            return Some(id);
        }
        let name = self.hosting.table.get(key).map_or("", |s| s.name.as_str());
        let id = self
            .editor
            .add_internal_buffer(format!("*todo: {name}*"), Mode::Fundamental);
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold_from_agents();
        }
        self.hosting.natives.map.get_mut(&key)?.todo.buffer = Some(id);
        Some(id)
    }

    /// Puts `shown` in strip `id`, with its faces.
    fn write_todo(&mut self, id: BufferId, shown: &Strip) {
        self.editor.set_internal_text(id, &shown.text);
        let mut highlights = Highlights::new(shown.text.len());
        for (range, face) in &shown.faces {
            highlights.replace(range.clone(), [(range.len(), Some(face.highlight()))]);
        }
        let _ = self.editor.set_internal_highlights(id, highlights);
    }
}
