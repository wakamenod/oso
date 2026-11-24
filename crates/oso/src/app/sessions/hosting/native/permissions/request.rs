//! A native session's call that waits for the person, in the app (ADR 0182): its block, the last
//! text of its session's transcript, and its line, `*request: NAME*`, docked between that and the
//! composer. Their texts are `sessions::native::request`'s; this puts them in, draws them again and
//! takes them away.
//!
//! Must not depend on: key presses or how a frame is drawn; the line's commands ask, and this and
//! its parts carry out.
//!
//! - The block's text under its title is put in once, and does not change until it goes; the title
//!   alone is drawn again, as the window's width or the count changes.
//! - The line is written again as what its keys do, or what was read, changes.
//! - Point goes to the line only as the person goes there: `C-c C-j`, `C-x o`, a click (until
//!   ADR 0182's "Point" 1 to 4 are built).
//! - What of the block was read is `read`'s, whether an answer counts `gate`'s, the keys `keys`',
//!   and the question before a call not all read runs `ask`'s.

mod ask;
mod gate;
mod keys;
mod read;

use std::time::Instant;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::syntax::Highlights;
use oso_core::window::DockSlot;

use crate::app::App;
use crate::app::sessions::hosting::native::{Native, NativeSessions, put_tail};
use crate::sessions::native::render::Rendered;
use crate::sessions::native::request::Block;
use crate::sessions::native::request::line::{Line, Says, line};
use crate::sessions::native::tail::{Part, Run};
use crate::sessions::worker::SessionKey;

/// What the shown calls' blocks and lines are drawn as, and the question asked last.
#[derive(Debug, Default)]
pub(super) struct Requests {
    drawn: Vec<Drawn>,
    /// The question before a call not all read runs, while it is open.
    asked: Option<ask::Asked>,
    /// Room for what a window shows.
    room: read::Room,
}

/// What session `key`'s block and line are drawn as.
#[derive(Debug)]
struct Drawn {
    key: SessionKey,
    /// Its title's place among the session's requests, and width.
    title: ((usize, usize), usize),
    /// What its line says, its width, and the line as written.
    line: Option<(Says, usize, Line)>,
    /// What was read changed since.
    stale: bool,
}

impl Requests {
    fn drawn(&mut self, key: SessionKey) -> Option<&mut Drawn> {
        self.drawn.iter_mut().find(|drawn| drawn.key == key)
    }
}

/// `rendered`'s faces as runs that cover its text.
fn runs(rendered: &Rendered) -> Vec<Run> {
    let mut runs = rendered.runs();
    let covered: usize = runs.iter().map(|run| run.0).sum();
    runs.push((rendered.text.len() - covered, None));
    runs
}

/// Puts `block`'s title in `native`'s tail, as `at` of `of` requests and `width` cells wide, under
/// the row that stands between it and what is above.
fn put_title(
    editor: &mut Editor,
    native: &mut Native,
    block: &Block,
    (place, width): ((usize, usize), usize),
) {
    let title = block.title(place, width);
    let its = [(1, None)].into_iter().chain(runs(&title)).collect();
    put_tail(
        editor,
        native,
        Part::Title,
        format!("\n{}", title.text),
        its,
    );
}

impl App {
    /// Session `key`'s call, just found, shows: its block and its line.
    pub(super) fn show_request(&mut self, key: SessionKey) {
        let place = (1, self.hosting.natives.permissions.asks.count(key));
        let NativeSessions {
            map, permissions, ..
        } = &mut self.hosting.natives;
        let shown = permissions.asks.shown(key).and_then(|ask| ask.request());
        let (Some(native), Some(request)) = (map.get_mut(&key), shown) else {
            return;
        };
        let title = (place, native.rows.width());
        // The rows in view wrap within a budget of their own (ADR 0103).
        self.editor.refill_wraps();
        put_title(&mut self.editor, native, &request.block, title);
        let body = request.block.body();
        let its = runs(body);
        put_tail(&mut self.editor, native, Part::Body, body.text.clone(), its);
        permissions.requests.drawn.retain(|drawn| drawn.key != key);
        (permissions.requests.drawn).push(Drawn {
            key,
            title,
            line: None,
            stale: false,
        });
        self.draw_line(key);
        let line = self.line_buffer(key);
        if let (Some(line), Some(native)) = (line, self.hosting.natives.map.get(&key)) {
            (self.editor).set_dock(native.buffer, DockSlot::Card, Some(line));
        }
        // Its width is known once it is docked.
        self.draw_line(key);
        self.request_redraw();
    }

    /// After each event: each shown call's title as its place and its window's width are, what its
    /// window shows looked at, and its line as both leave it.
    pub(in crate::app) fn sync_requests(&mut self) {
        if self.hosting.natives.permissions.requests.drawn.is_empty() {
            return;
        }
        self.note_current_card();
        for at in 0..self.hosting.natives.permissions.requests.drawn.len() {
            let Some(drawn) = self.hosting.natives.permissions.requests.drawn.get(at) else {
                break;
            };
            let (key, before) = (drawn.key, drawn.title);
            let place = (1, self.hosting.natives.permissions.asks.count(key));
            let NativeSessions {
                map, permissions, ..
            } = &mut self.hosting.natives;
            let shown = permissions.asks.shown(key).and_then(|ask| ask.request());
            if let (Some(native), Some(request)) = (map.get_mut(&key), shown) {
                let title = (place, native.rows.width());
                if title != before {
                    put_title(&mut self.editor, native, &request.block, title);
                    if let Some(drawn) = permissions.requests.drawn(key) {
                        drawn.title = title;
                    }
                    self.request_redraw();
                }
            }
            self.look_at_request(key, Instant::now());
            self.draw_line(key);
        }
    }

    /// Writes session `key`'s line, if it says another thing than as drawn last.
    fn draw_line(&mut self, key: SessionKey) {
        let Some(buffer) = self.line_buffer(key) else {
            return;
        };
        let says = self.line_says(key, buffer);
        let width = self.line_width(key);
        let permissions = &mut self.hosting.natives.permissions;
        let Some(request) = permissions.asks.shown(key).and_then(|ask| ask.request()) else {
            return;
        };
        let Some(drawn) = permissions.requests.drawn(key) else {
            return;
        };
        let before = drawn.line.as_ref();
        // Out of sight it keeps the width it had.
        let width = width.or(before.map(|(_, width, _)| *width));
        let width = width.unwrap_or(super::card::UNSHOWN_WIDTH);
        if !drawn.stale && before.is_some_and(|before| (before.0, before.1) == (says, width)) {
            return;
        }
        let line = line(&request.block, &request.reading, says, width);
        drawn.stale = false;
        let same = before.is_some_and(|(_, _, before)| *before == line);
        let lit = (self.editor.buffer(buffer)).is_some_and(|b| b.highlights().is_some());
        let kept = drawn.line.insert((says, width, line));
        if same && lit {
            return;
        }
        let shown = &kept.2.shown;
        self.editor.set_internal_text(buffer, &shown.text);
        let mut highlights = Highlights::new(shown.text.len());
        highlights.replace(0..shown.text.len(), runs(shown));
        let _ = self.editor.set_internal_highlights(buffer, highlights);
        self.request_redraw();
    }

    /// Session `key`'s line's buffer, made the first time.
    fn line_buffer(&mut self, key: SessionKey) -> Option<BufferId> {
        let native = self.hosting.natives.map.get(&key)?;
        if let Some(id) = native.line.filter(|&id| self.editor.buffer(id).is_some()) {
            return Some(id);
        }
        let name = self.hosting.table.get(key).map_or("", |s| s.name.as_str());
        let id = self
            .editor
            .add_internal_buffer(format!("*request: {name}*"), Mode::Request);
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold_from_agents();
        }
        self.hosting.natives.map.get_mut(&key)?.line = Some(id);
        Some(id)
    }

    /// Session `key`'s call was answered: its block and its line go, or the session's next call
    /// shows in their place, at once, with point where it was.
    pub(super) fn close_request(&mut self, key: SessionKey) {
        self.forget_request(key);
        let next = (self.hosting.natives.permissions.asks.unshown())
            .find(|ask| ask.key == key)
            .is_some_and(|ask| ask.request().is_some());
        if next {
            return self.show_asked();
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let (transcript, line) = (native.buffer, native.line);
        for part in [Part::Title, Part::Body] {
            put_tail(&mut self.editor, native, part, String::new(), Vec::new());
        }
        let was_in = line.is_some_and(|line| self.editor.window().buffer() == line);
        self.editor.set_dock(transcript, DockSlot::Card, None);
        if let Some(line) = line {
            self.editor.set_internal_text(line, "");
        }
        if was_in {
            self.composer_go(key, true);
        }
        self.request_redraw();
    }

    /// Session `key`'s call of `tool` went otherwise than by an answer of the person's.
    pub(super) fn request_gone(&mut self, key: SessionKey, tool: &str) {
        let name = (self.hosting.table.get(key)).map_or_else(String::new, |s| s.name.clone());
        self.editor
            .message(format!("{tool} no longer waits for you in session {name}"));
        self.close_request(key);
    }

    /// Nothing of session `key`'s call is asked any more: its question, and what is written for it.
    fn forget_request(&mut self, key: SessionKey) {
        let permissions = &mut self.hosting.natives.permissions;
        permissions.requests.drawn.retain(|drawn| drawn.key != key);
        if (permissions.writing.as_ref()).is_some_and(|(writing, _)| writing.session() == key) {
            permissions.writing = None;
        }
        self.drop_unread_question(key);
    }

    /// Whether `buffer` is a request's line, and whose.
    pub(in crate::app::sessions::hosting::native) fn line_session(
        &self,
        buffer: BufferId,
    ) -> Option<SessionKey> {
        (self.hosting.natives.map.iter())
            .find_map(|(key, native)| (native.line == Some(buffer)).then_some(*key))
    }
}

#[cfg(test)]
mod tail_tests;
