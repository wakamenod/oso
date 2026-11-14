//! What the app sends a terminal's program besides clicks and drags (ADR 0120): keys, text the
//! input method committed, pastes, which ask first for lines a program may run, and the wheel,
//! which goes to the program when it asked for it (`mouse`'s `wheel_to_program`) and otherwise
//! scrolls the scrollback.
//!
//! Must not depend on: how a frame is drawn.

use oso_core::buffer::BufferId;
use oso_core::key::KeyPress;
use oso_term::{QueueFull, Scroll};

use super::super::App;
use crate::terminal::paste;

impl App {
    pub(super) fn send_keys(&mut self, buffer: BufferId, keys: &[KeyPress]) {
        let Some(terminal) = self
            .terminals
            .map
            .get(buffer)
            .and_then(|hosted| hosted.terminal.as_ref())
        else {
            return self.no_program(buffer);
        };
        for key in keys {
            if terminal.send_key(*key) == Err(QueueFull) {
                self.editor
                    .message("The terminal's input is full; key dropped");
                return;
            }
        }
    }

    /// Text the input method committed, or a key that types several characters, for the
    /// terminal `buffer`'s program, as UTF-8.
    pub(super) fn send_text(&mut self, buffer: BufferId, text: &str) {
        let Some(terminal) = self
            .terminals
            .map
            .get(buffer)
            .and_then(|hosted| hosted.terminal.as_ref())
        else {
            return self.no_program(buffer);
        };
        if terminal.send(text.as_bytes().to_vec()).is_err() {
            self.editor
                .message("The terminal's input is full; text dropped");
        }
    }

    pub(super) fn no_program(&mut self, buffer: BufferId) {
        let said = match self.terminals.map.get(buffer) {
            Some(hosted) if hosted.ended.is_some() => "No program runs in this terminal",
            Some(_) => "The terminal is starting",
            None => return,
        };
        self.editor.message(said);
    }

    /// `C-y` in a terminal: with bracketed paste off, text of several lines asks first
    /// (ADR 0120, Q13).
    pub(super) fn paste(&mut self, buffer: BufferId, text: String) {
        let Some(hosted) = self.terminals.map.get_mut(buffer) else {
            return;
        };
        if paste::breaks(&text) && !hosted.grid.modes().bracketed_paste {
            let lines = paste::lines(&text);
            let question = format!("Paste {lines} lines into a program that may run them?");
            match self
                .editor
                .ask_with(question, "terminal-paste-lines", ["paste", "keep"])
            {
                Ok(asked) => {
                    if let Some(hosted) = self.terminals.map.get_mut(buffer) {
                        hosted.pending_paste = Some((asked, text));
                    }
                }
                Err(error) => self.editor.message(error.to_string()),
            }
            self.request_redraw();
            return;
        }
        self.send_paste(buffer, text, false);
    }

    pub(super) fn send_paste(&mut self, buffer: BufferId, text: String, lines_allowed: bool) {
        let Some(terminal) = self
            .terminals
            .map
            .get(buffer)
            .and_then(|hosted| hosted.terminal.as_ref())
        else {
            return self.no_program(buffer);
        };
        if terminal.paste(text, lines_allowed).is_err() {
            self.editor
                .message("The terminal's input is full; nothing pasted");
        }
    }

    /// The wheel over a terminal, `rows` down and `columns` right with the pointer `at`: to
    /// its program if it wants it (step 4b-2), or over its scrollback. Returns whether the
    /// window shows a terminal.
    pub(in crate::app) fn wheel_terminal(
        &mut self,
        window: oso_core::window::WindowId,
        (rows, columns): (isize, isize),
        at: Option<oso_platform::window::Position>,
    ) -> bool {
        let Some(buffer) = self.editor.window_by_id(window).map(|w| w.buffer()) else {
            return false;
        };
        if self.terminals.map.get(buffer).is_none() {
            return false;
        }
        if self.wheel_to_program(window, buffer, rows, columns, at) {
            return true;
        }
        let Some(hosted) = self.terminals.map.get(buffer) else {
            return false;
        };
        if let Some(terminal) = &hosted.terminal
            && rows != 0
        {
            terminal.scroll(Scroll::Lines(rows));
        }
        true
    }
}
