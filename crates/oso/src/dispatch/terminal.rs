//! Keys in a terminal (ADR 0120, Q12): before any keymap, a key the terminal's mode does not
//! keep for oso goes to its program, through the host. Each terminal's mode, and whether a
//! `C-q` or char mode's `C-c` waits, is held here, as key-reading state.
//!
//! Must not depend on: `app` or the terminal's thread. A key for the program becomes a
//! request the host takes after the event. Keys sent to a program are not recorded into a
//! keyboard macro (the author's choice on 2026-11-13): until echo off is told apart, a
//! password typed at `sudo` would enter it.

use oso_core::buffer::BufferId;
use oso_core::command::Effects;
use oso_core::editor::{Editor, TerminalRequest};
use oso_core::key::KeyPress;
use oso_core::keymap::is_cancel;

use crate::terminal::route::{self, Input, Route, State};

use super::Dispatcher;

impl Dispatcher {
    /// Sends `key` to the selected terminal's program if it is not oso's there. `None` when
    /// no terminal is selected or the key is oso's, for the keymaps to resolve.
    pub(super) fn route_to_terminal(
        &mut self,
        editor: &mut Editor,
        key: KeyPress,
    ) -> Option<Effects> {
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
            editor.request_terminal(TerminalRequest::Keys(vec![route::C_C, key]));
            return Some(Effects::default());
        }
        let busy = !self.resolver.pending().is_empty()
            || self.prefix.is_some()
            || self.waiting.is_some()
            || self.menus.is_open()
            || editor.isearch().is_some()
            || editor.jump().is_some()
            || editor.replace().is_some();
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
                editor.request_terminal(TerminalRequest::Keys(keys));
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
    }
}
