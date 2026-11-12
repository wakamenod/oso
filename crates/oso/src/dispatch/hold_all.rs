//! Input held while work goes on a slice a turn: query-replace's `!` (ADR 0113), and an undo
//! or a redo of a large step (ADR 0119). `C-g` stops `!`, keeping what it replaced, or cancels
//! the undo, changing nothing; anything else waits, and runs in order once it has ended. The
//! window's close button waits with the keys.
//!
//! Must not depend on: `app` or winit; the host asks for each slice and says when to go on.

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};

use oso_core::buffer::EditError;
use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::is_cancel;

use super::Dispatcher;
use crate::emergency::Publisher;

/// What the echo area says when a slice of an undo panicked; the payload is dropped unread
/// (ADR 0028).
const PANICKED: &str = "Undo or redo panicked";

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
    /// `C-g` dropped what was held, and cancelled an undo or a redo or stopped `!`, since the
    /// host last asked.
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
            self.held_during_all.cancelled = true;
            self.held_during_all.held.clear();
            return Some(Effects {
                redraw: true,
                ..Effects::default()
            });
        }
        // A close request is never dropped, however many keys came before it.
        if self.held_during_all.held.len() < MOST_HELD || matches!(input, Held::Close) {
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

    /// Whether `C-g` dropped what was held since the last call, cancelling an undo or a redo or
    /// stopping `!`: what the host holds goes too, and the agent whose command began the undo
    /// hears it (ADR 0119).
    pub fn take_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.held_during_all.cancelled)
    }

    /// Runs the next slice of the undo or the redo under way, outside a command: the wraps get
    /// their budget first and the windows follow their cursors after, as around a command.
    /// Once it has ended, an error is said, and stops a keyboard macro's replay, as a failing
    /// command does, and the input held meanwhile runs. Returns what the host should do, and
    /// how it ended, if it did (ADR 0119). It asks for a frame only when the text or a view
    /// moved, or the crossing ended.
    pub fn go_on_crossing(
        &mut self,
        editor: &mut Editor,
    ) -> (Effects, Option<Result<(), EditError>>) {
        if editor.crossing().is_none() {
            return (Effects::default(), None);
        }
        let left = editor.crossing().map(oso_core::editor::Crossing::left);
        editor.refill_wraps();
        // A panic in a slice drops the crossing and nothing else, as the editor holds none
        // while a slice runs; outside a command, this is its boundary (ADR 0014, ADR 0119).
        let boundary = self.publisher.as_ref().map(Publisher::enter_command);
        let ran = panic::catch_unwind(AssertUnwindSafe(|| editor.go_on_crossing()));
        drop(boundary);
        let (ended, panicked) = match ran {
            Ok(ended) => (ended, false),
            Err(_) => (Some(Err(EditError::Abandoned)), true),
        };
        if panicked {
            editor.message(PANICKED);
            if let Some(publisher) = &self.publisher {
                publisher.command_panicked(editor);
            }
        }
        let moved = editor.follow_cursor();
        // The text changes only as a step commits; the host's pace draws the progress.
        let committed = editor.crossing().map(oso_core::editor::Crossing::left) != left;
        let mut effects = Effects {
            redraw: moved || committed || ended.is_some(),
            ..Effects::default()
        };
        if let Some(result) = &ended {
            if let Err(error) = result {
                if !panicked {
                    editor.message(error.to_string());
                }
                editor.macro_stop(None);
            }
            effects = effects.union(self.run_held_during_all(editor));
        }
        (effects, ended)
    }
}
