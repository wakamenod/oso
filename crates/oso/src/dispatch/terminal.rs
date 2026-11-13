//! Keys in a terminal (ADR 0120, Q12): before any keymap, a key the terminal's mode does not
//! keep for oso goes to its program, through the host. Each terminal's mode, and whether a
//! `C-q` or char mode's `C-c` waits, is held here, as key-reading state.
//!
//! Must not depend on: `app` or the terminal's thread. What goes to a program waits here, each
//! send bound to the terminal it was meant for, until the host takes it after the event, so that
//! a batch of keys that moves to another window sends nothing to the wrong one. Keys sent to a
//! program are not recorded into a keyboard macro (the author's choice on 2026-11-13): until
//! echo off is told apart, a password typed at `sudo` would enter it.

use oso_core::buffer::BufferId;
use oso_core::command::Effects;
use oso_core::editor::{Editor, TerminalRequest};
use oso_core::key::KeyPress;
use oso_core::keymap::is_cancel;

use crate::terminal::route::{self, Input, Route, State};

use super::Dispatcher;

/// What waits to go to a terminal's program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Send {
    Keys(Vec<KeyPress>),
    /// Text an input method committed, or a key that types several characters.
    Text(String),
    Paste(String),
    /// The person said yes to pasting lines without bracketed paste.
    PasteLines,
}

impl Dispatcher {
    /// Sends `key` to the selected terminal's program if it is not oso's there. `None` when
    /// no terminal is selected or the key is oso's, for the keymaps to resolve.
    pub(super) fn route_to_terminal(
        &mut self,
        editor: &mut Editor,
        key: KeyPress,
    ) -> Option<Effects> {
        self.take_key_requests(editor);
        let (window, buffer) = editor.current();
        if !buffer.is_terminal() || editor.prompt().is_some() {
            return None;
        }
        let id = window.buffer();
        // `C-c` and a key the terminal's keymap does not bind after it send both (ADR 0120).
        let pending_c_c = self.resolver.pending() == [route::C_C] && self.prefix.is_none();
        if pending_c_c
            && !is_cancel(key)
            && self.keymaps.terminal.lookup(&[route::C_C, key]).is_none()
        {
            self.resolver.reset();
            self.forget_keys(editor);
            // The `C-c` a recording noted went to the program too.
            self.macro_forget_input();
            self.terminal_sends
                .push((id, Send::Keys(vec![route::C_C, key])));
            return Some(Effects::default());
        }
        let busy = self.oso_busy(editor);
        let state = self.terminal_keys.entry(id).or_default();
        let routed = route::route(
            State {
                oso_busy: busy,
                ..*state
            },
            key,
        );
        if routed != Route::Oso {
            state.quoted = false;
            state.held_c_c = false;
        }
        match routed {
            Route::Oso => None,
            Route::Send(keys) => {
                self.terminal_sends.push((id, Send::Keys(keys)));
                Some(Effects::default())
            }
            Route::Hold => {
                state.held_c_c = true;
                Some(Effects::default())
            }
            Route::LeaveCharMode => {
                state.input = Input::SemiChar;
                Some(Effects {
                    redraw: true,
                    ..Effects::default()
                })
            }
        }
    }

    /// Something of oso's is open that takes every key: a key sequence or a prefix argument in
    /// progress, the clipboard awaited, a menu, isearch, a jump or a query-replace.
    fn oso_busy(&self, editor: &Editor) -> bool {
        !self.resolver.pending().is_empty()
            || self.prefix.is_some()
            || self.waiting.is_some()
            || self.menus.is_open()
            || editor.isearch().is_some()
            || editor.jump().is_some()
            || editor.replace().is_some()
    }

    /// Sends `text`, an input method's commit or a key's several characters, to the selected
    /// terminal's program, as a key would go: not while something of oso's is open, and after
    /// a `C-c` char mode held. Returns whether it went there.
    pub fn text_to_terminal(&mut self, editor: &mut Editor, text: &str) -> bool {
        self.take_key_requests(editor);
        let (window, buffer) = editor.current();
        if !buffer.is_terminal() || editor.prompt().is_some() || self.oso_busy(editor) {
            return false;
        }
        let id = window.buffer();
        let state = self.terminal_keys.entry(id).or_default();
        if state.held_c_c {
            self.terminal_sends.push((id, Send::Keys(vec![route::C_C])));
        }
        state.held_c_c = false;
        state.quoted = false;
        self.terminal_sends.push((id, Send::Text(text.to_owned())));
        true
    }

    /// Binds what commands asked of the selected terminal to it, at once: `C-q` and `C-c C-k`
    /// change how the very next key is read, from the same batch of the dev socket or a
    /// macro's replay, and keys or a paste go to the terminal the command ran in, whatever is
    /// selected when the host acts. Starting a terminal stays for the host.
    pub(super) fn take_key_requests(&mut self, editor: &mut Editor) {
        let requests = editor.take_terminal_requests();
        if requests.is_empty() {
            return;
        }
        let selected = editor.window().buffer();
        for request in requests {
            match request {
                TerminalRequest::QuoteNext => self.quote_next(selected),
                TerminalRequest::ToggleCharMode => self.toggle_char_mode(selected),
                TerminalRequest::Keys(keys) => {
                    self.terminal_sends.push((selected, Send::Keys(keys)))
                }
                TerminalRequest::Paste { text } => {
                    self.terminal_sends.push((selected, Send::Paste(text)));
                }
                TerminalRequest::PasteLines => {
                    self.terminal_sends.push((selected, Send::PasteLines));
                }
                TerminalRequest::Open { .. } => editor.request_terminal(request),
            }
        }
    }

    /// What waits to go to terminals' programs, each with its terminal, oldest first. What
    /// commands asked is bound to the selected terminal first.
    pub fn take_terminal_sends(&mut self, editor: &mut Editor) -> Vec<(BufferId, Send)> {
        self.take_key_requests(editor);
        std::mem::take(&mut self.terminal_sends)
    }

    /// How the terminal `buffer` reads keys.
    pub fn terminal_input(&self, buffer: BufferId) -> Input {
        self.terminal_keys
            .get(&buffer)
            .map_or(Input::SemiChar, |state| state.input)
    }

    /// Switches the terminal `buffer` between semi-char and char mode (`C-c C-k`).
    pub fn toggle_char_mode(&mut self, buffer: BufferId) {
        let state = self.terminal_keys.entry(buffer).or_default();
        state.input = match state.input {
            Input::SemiChar => Input::Char,
            Input::Char => Input::SemiChar,
        };
        state.held_c_c = false;
    }

    /// Sends the next key in the terminal `buffer` to its program as it is (`C-q`).
    pub fn quote_next(&mut self, buffer: BufferId) {
        self.terminal_keys.entry(buffer).or_default().quoted = true;
    }

    /// Forgets the key state of terminals that are gone.
    pub fn forget_terminals(&mut self, mut alive: impl FnMut(BufferId) -> bool) {
        self.terminal_keys.retain(|id, _| alive(*id));
        self.terminal_sends.retain(|(id, _)| alive(*id));
    }
}
