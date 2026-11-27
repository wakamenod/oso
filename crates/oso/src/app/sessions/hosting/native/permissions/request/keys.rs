//! The keys of a request's line carried out (ADR 0182, "The line"): `y` allows the call once all of
//! it was read, `n` and `N` deny it, `C-u y` asks about what was not read, and the others show more of
//! it or leave it. And `C-c C-j`, which goes to it.
//!
//! Must not depend on: key presses or how a frame is drawn; the commands ask, and this carries out.
//!
//! - Every answer passes `gate`, and is of the call that is shown then: what point is on is not
//!   looked at.
//! - The transcript scrolls in the window that carries the line, and point stays on the line.

use oso_core::buffer::BufferId;
use oso_core::editor::NativeAction;
use oso_core::window::{DockSlot, Scroll, WindowId, screen_lines};

use super::gate::Pressed;
use crate::app::App;
use crate::app::sessions::hosting::native::carrier::carrier;
use crate::app::sessions::hosting::native::permissions::DENIED;
use crate::app::sessions::hosting::native::permissions::card::{
    Answer, BY_THE_PERSON, By, Writing,
};
use crate::sessions::native::request::input::input;
use crate::sessions::native::tail::Part;
use crate::sessions::worker::SessionKey;

/// What `C-c C-j` says where the request does not show.
const HIDDEN: &str = "The request is hidden: the window is too short for it";

/// Which place of a block is shown.
#[derive(Clone, Copy)]
enum At {
    /// Its title.
    Title,
    /// A byte of its body.
    Body(usize),
}

impl App {
    /// A key of session `key`'s line, pressed in it, `line`: `action`. Whether it was one of the
    /// line's.
    pub(in crate::app::sessions::hosting::native) fn request_key(
        &mut self,
        key: SessionKey,
        line: BufferId,
        action: NativeAction,
    ) -> bool {
        let answers = matches!(
            action,
            NativeAction::Allow | NativeAction::Unread | NativeAction::Deny | NativeAction::Note
        );
        if !answers && !matches!(action, NativeAction::ReadOn(_) | NativeAction::Open) {
            return false;
        }
        let shown = self.hosting.natives.permissions.asks.shown(key);
        if shown.is_none_or(|ask| ask.request().is_none()) {
            self.editor.message("No call waits for you here");
            return true;
        }
        match action {
            NativeAction::ReadOn(pages) => self.read_on(key, pages),
            NativeAction::Open => self.show_input(key),
            _ => {
                if let Some(pressed) = self.request_admits(key, line) {
                    self.answer_request(key, action, pressed);
                }
            }
        }
        self.request_redraw();
        true
    }

    /// Session `key`'s line takes an answer of the key `pressed`, `action`.
    fn answer_request(&mut self, key: SessionKey, action: NativeAction, pressed: Pressed) {
        match action {
            NativeAction::Allow => self.allow_request(key, pressed),
            NativeAction::Unread => self.allow_unread(key, pressed),
            NativeAction::Note => self.write_for_card(Writing::Note(key)),
            _ => {
                let denied = Answer::Deny(DENIED.to_owned(), BY_THE_PERSON.to_owned());
                self.answer_card(key, denied);
            }
        }
    }

    /// `y` on session `key`'s line, which takes an answer: the call is allowed if all of it was
    /// read, its last row in view; else what is left to read is shown, and nothing is answered.
    fn allow_request(&mut self, key: SessionKey, pressed: Pressed) {
        self.look_at_request(key, pressed.at);
        let shown = self.hosting.natives.permissions.asks.shown(key);
        let Some(reading) = shown.and_then(|ask| ask.request()).map(|r| &r.reading) else {
            return;
        };
        if reading.allows() {
            return self.answer_card(key, Answer::Allow(By::Person));
        }
        let (unread, in_view, whole) = (
            reading.unread(),
            reading.unread_in_view(),
            reading.all_read(),
        );
        match unread {
            Some(_) if in_view => self
                .editor
                .message("Not read yet: wait a moment, then answer"),
            Some(at) => {
                self.show_in_block(key, At::Body(at));
                self.editor
                    .message("Not allowed: here is what was not read. C-u y allows it unread");
            }
            None if whole => {
                self.read_on(key, isize::MAX);
                self.editor
                    .message("Not allowed: here is its end. y allows it now");
            }
            None => {
                self.editor
                    .message("Not all of it is shown: C-u y allows it unread");
            }
        }
    }

    /// `C-u y` on session `key`'s line, which takes an answer: it asks about what was not read, once
    /// some of the block was in view for long enough; a call all read is allowed as `y` allows it.
    fn allow_unread(&mut self, key: SessionKey, pressed: Pressed) {
        self.look_at_request(key, pressed.at);
        let shown = self.hosting.natives.permissions.asks.shown(key);
        let Some(reading) = shown.and_then(|ask| ask.request()).map(|r| &r.reading) else {
            return;
        };
        if reading.all_read() {
            return self.allow_request(key, pressed);
        }
        if !reading.may_ask() {
            return self
                .editor
                .message("Wait until some of it was in view for a moment");
        }
        self.ask_unread(key);
    }

    /// `SPC`, `DEL` and the block's ends: the window that carries session `key`'s line shows the
    /// transcript `pages` pages on. With nothing more below, what is left to read above.
    fn read_on(&mut self, key: SessionKey, pages: isize) {
        match pages {
            isize::MIN => return self.show_in_block(key, At::Title),
            isize::MAX => return self.show_end(key),
            _ => {}
        }
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let Some((id, _)) = carrier(&self.editor, native.buffer, DockSlot::Card) else {
            return;
        };
        let Some(page) = self.editor.window_by_id(id).map(|w| screen_lines(w.size())) else {
            return;
        };
        let rows = isize::try_from(page)
            .unwrap_or(isize::MAX)
            .saturating_mul(pages);
        if self.editor.scroll_view(id, rows) {
            return;
        }
        let shown = self.hosting.natives.permissions.asks.shown(key);
        let reading = shown.and_then(|ask| ask.request()).map(|r| &r.reading);
        let above = reading.filter(|reading| pages > 0 && !reading.unread_in_view());
        match above.and_then(|reading| reading.unread()) {
            Some(at) => self.show_in_block(key, At::Body(at)),
            None => self.editor.message("No more rows that way"),
        }
    }

    /// The window that carries session `key`'s line shows `at` of its block at its top, or its
    /// block's end at its bottom if less than a window is below.
    fn show_in_block(&mut self, key: SessionKey, at: At) {
        let Some((id, offset)) = self.place_in_block(key, at) else {
            return;
        };
        let top = Scroll {
            start: offset,
            left: 0,
        };
        self.editor.refill_wraps();
        super::head::put(&mut self.editor, id, offset, top);
        // The rows below it may be fewer than the window's.
        let blank = self.editor.window_by_id(id).and_then(|window| {
            let display = self.editor.buffer(window.buffer())?.display();
            let below = (window.row_count(&display)).saturating_sub(window.top_row(&display));
            isize::try_from(window.size().rows.saturating_sub(below)).ok()
        });
        if let Some(rows) = blank.filter(|rows| *rows > 0) {
            self.editor.scroll_view(id, -rows);
        }
    }

    /// The window that carries session `key`'s line, and the byte of its transcript at `at` of its
    /// block.
    fn place_in_block(&self, key: SessionKey, at: At) -> Option<(WindowId, usize)> {
        let native = self.hosting.natives.map.get(&key)?;
        let (id, _) = carrier(&self.editor, native.buffer, DockSlot::Card)?;
        let offset = match at {
            // Past the row between it and what is above.
            At::Title | At::Body(0) => super::head::first(native),
            At::Body(at) => native.part_place(Part::Body).start + at,
        };
        Some((id, offset))
    }

    /// The window that carries session `key`'s line shows its block's end, and follows it.
    fn show_end(&mut self, key: SessionKey) {
        let Some((id, _)) = self.place_in_block(key, At::Title) else {
            return;
        };
        let end = (self.hosting.natives.map.get(&key))
            .and_then(|native| self.editor.buffer(native.buffer))
            .map(|buffer| buffer.text().len());
        let scroll = self.editor.window_by_id(id).map(|window| window.scroll());
        if let (Some(end), Some(scroll)) = (end, scroll) {
            self.editor.refill_wraps();
            super::head::put(&mut self.editor, id, end, scroll);
        }
    }

    /// `RET` on session `key`'s line: its call's input as JSON in `*result: NAME*`, as a result is.
    fn show_input(&mut self, key: SessionKey) {
        let name = (self.hosting.table.get(key)).map_or_else(String::new, |s| s.name.clone());
        let shown = self.hosting.natives.permissions.asks.shown(key);
        let Some((ask, request)) = shown.and_then(|ask| Some((ask, ask.request()?))) else {
            return;
        };
        let text = input(&name, request.block.tool(), ask.call.input.as_ref());
        self.show_request_input(key, &name, &text);
    }

    /// `C-c C-j`: point goes to session `key`'s request, its line or its card.
    pub(in crate::app::sessions::hosting::native) fn go_to_request(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        if self.hosting.natives.permissions.asks.shown(key).is_none() {
            return self.editor.message("Nothing waits for you here");
        }
        let shown = self.hosting.natives.permissions.asks.shown(key);
        if shown.is_some_and(|ask| ask.request().is_some()) {
            match self.select_line(key) {
                true => self.note_current_card(),
                false => self.editor.message(HIDDEN),
            }
            return;
        }
        let dock = carrier(&self.editor, native.buffer, DockSlot::Card)
            .and(self.editor.dock_window(native.buffer, DockSlot::Card));
        match dock {
            Some(window) => {
                self.editor.select_window(window);
                self.note_current_card();
            }
            None => self.editor.message(HIDDEN),
        }
    }

    /// The window the wheel scrolls over window `id`: the one that carries it, if it is a request's
    /// line.
    pub(in crate::app) fn wheel_over_request(&self, id: WindowId) -> WindowId {
        let line = (self.editor.window_by_id(id))
            .and_then(|window| Some((window.docked()?, self.line_session(window.buffer())?)));
        let Some(((host, slot), _)) = line else {
            return id;
        };
        carrier(&self.editor, host, slot).map_or(id, |(carrier, _)| carrier)
    }
}
