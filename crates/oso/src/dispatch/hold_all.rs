//! Input held while work goes on a slice a turn: query-replace's `!` (ADR 0113), and an undo
//! or a redo of a large step (ADR 0119). `C-g` stops `!`, keeping what it replaced, or cancels
//! the undo, changing nothing; anything else waits, and runs in order once it has ended. The
//! window's close button waits with the keys.
//!
//! Must not depend on: `app` or winit; the host asks for each slice and says when to go on.

use std::collections::VecDeque;

use oso_core::buffer::EditError;
use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::is_cancel;

use super::Dispatcher;

/// The most input held while work goes on; past it, input is dropped, as a key held too long.
const MOST_HELD: usize = 256;

/// One input held while work goes on.
pub(super) enum Held {
    Key(KeyPress),
    Text {
        text: String,
        commit: bool,
    },
    /// The window's close button.
    Close,
}

/// The input held while work goes on.
#[derive(Default)]
pub(super) struct HeldDuringAll {
    held: VecDeque<Held>,
    /// `C-g` cancelled an undo or a redo since the host last asked.
    cancelled: bool,
}

/// Whether input waits now: while `!` replaces or an undo or a redo goes on.
pub fn holds_input(editor: &Editor) -> bool {
    editor.replace_works() || editor.crossing().is_some()
}

impl Dispatcher {
    /// Holds `input` while work goes on, or stops it for `C-g`. Returns what it did, if it
    /// took the input.
    pub(super) fn hold_during_all(&mut self, editor: &mut Editor, input: Held) -> Option<Effects> {
        if !holds_input(editor) {
            return None;
        }
        if matches!(input, Held::Key(key) if is_cancel(key)) {
            match editor.crossing().map(oso_core::editor::Crossing::redo) {
                Some(redo) => {
                    editor.cancel_crossing();
                    self.held_during_all.cancelled = true;
                    editor.message(if redo {
                        "Redo cancelled"
                    } else {
                        "Undo cancelled"
                    });
                }
                None => {
                    editor.replace_exit();
                }
            }
            self.held_during_all.held.clear();
            return Some(Effects {
                redraw: true,
                ..Effects::default()
            });
        }
        if self.held_during_all.held.len() < MOST_HELD {
            self.held_during_all.held.push_back(input);
        }
        Some(Effects::default())
    }

    /// Runs the input held while work went on, once it has ended. Returns what the host should
    /// do.
    pub fn run_held_during_all(&mut self, editor: &mut Editor) -> Effects {
        let mut effects = Effects::default();
        while !holds_input(editor) {
            let Some(input) = self.held_during_all.held.pop_front() else {
                break;
            };
            effects = effects.union(match input {
                Held::Key(key) => self.handle_key(editor, key),
                Held::Text { text, commit: true } => self.commit_text(editor, &text),
                Held::Text {
                    text,
                    commit: false,
                } => self.insert_text(editor, &text),
                Held::Close => self.close_requested(editor),
            });
        }
        effects
    }

    /// Whether `C-g` cancelled an undo or a redo since the last call, for the agent whose
    /// command began it (ADR 0119).
    pub fn take_crossing_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.held_during_all.cancelled)
    }

    /// Runs the next slice of the undo or the redo under way, outside a command: the wraps get
    /// their budget first and the windows follow their cursors after, as around a command.
    /// Once it has ended, an error is said, and stops a keyboard macro's replay, as a failing
    /// command does, and the input held meanwhile runs. Returns what the host should do, and
    /// how it ended, if it did (ADR 0119).
    pub fn go_on_crossing(
        &mut self,
        editor: &mut Editor,
    ) -> (Effects, Option<Result<(), EditError>>) {
        if editor.crossing().is_none() {
            return (Effects::default(), None);
        }
        editor.refill_wraps();
        let ended = editor.go_on_crossing();
        editor.follow_cursor();
        let mut effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if let Some(result) = &ended {
            if let Err(error) = result {
                editor.message(error.to_string());
                editor.macro_stop(None);
            }
            effects = effects.union(self.run_held_during_all(editor));
        }
        (effects, ended)
    }
}
