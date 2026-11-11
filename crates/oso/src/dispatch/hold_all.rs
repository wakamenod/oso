//! Input held while query-replace's `!` replaces a slice a turn (ADR 0113): `C-g` stops it,
//! keeping what it replaced, and anything else waits, and runs once it has ended.
//!
//! Must not depend on: `app` or winit; the host says when `!` has ended.

use std::collections::VecDeque;

use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::is_cancel;

use super::Dispatcher;

/// The most input held while `!` runs; past it, input is dropped, as a key held too long.
const MOST_HELD: usize = 256;

/// One input held while `!` runs.
pub(super) enum Held {
    Key(KeyPress),
    Text { text: String, commit: bool },
}

/// The input held while `!` runs.
#[derive(Default)]
pub(super) struct HeldDuringAll(VecDeque<Held>);

impl Dispatcher {
    /// Holds `input` while `!` runs, or stops `!` for `C-g`. Returns what it did, if it took
    /// the input.
    pub(super) fn hold_during_all(&mut self, editor: &mut Editor, input: Held) -> Option<Effects> {
        if !editor.replace_works() {
            return None;
        }
        if matches!(input, Held::Key(key) if is_cancel(key)) {
            editor.replace_exit();
            self.held_during_all.0.clear();
            return Some(Effects {
                redraw: true,
                ..Effects::default()
            });
        }
        if self.held_during_all.0.len() < MOST_HELD {
            self.held_during_all.0.push_back(input);
        }
        Some(Effects::default())
    }

    /// Runs the input held while `!` ran, once it has ended. Returns what the host should do.
    pub fn run_held_during_all(&mut self, editor: &mut Editor) -> Effects {
        let mut effects = Effects::default();
        while !editor.replace_works() {
            let Some(input) = self.held_during_all.0.pop_front() else {
                break;
            };
            effects = effects.union(match input {
                Held::Key(key) => self.handle_key(editor, key),
                Held::Text { text, commit: true } => self.commit_text(editor, &text),
                Held::Text {
                    text,
                    commit: false,
                } => self.insert_text(editor, &text),
            });
        }
        effects
    }
}
