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
use oso_core::minibuffer::PromptId;

use super::Dispatcher;
use crate::emergency::Publisher;

/// What the echo area says when a slice of an undo panicked; the payload is dropped unread
/// (ADR 0028).
const PANICKED: &str = "Undo or redo panicked";

/// The most input held while work goes on; past it, input is dropped, as a key held too long.
const MOST_HELD: usize = 256;

/// What the echo area says when a prompt opened while input was held: the keys typed for
/// what was in front of the person then never answer it.
const DROPPED: &str = "Keys typed meanwhile dropped: a prompt opened";

/// One input held while work goes on, with whether it came through the dev socket, which never
/// reads the system clipboard.
pub(super) enum Held {
    Key(KeyPress, bool),
    Text {
        text: String,
        commit: bool,
        from_socket: bool,
    },
    /// The window's close button.
    Close,
}

/// What `C-g` stopped while input was held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cancelled {
    /// `!`, keeping what it replaced.
    Replace,
    /// An undo or a redo, with the steps of its count still to cross after the one cancelled.
    Crossing { left: usize },
}

/// The input held while work goes on.
#[derive(Default)]
pub(super) struct HeldDuringAll {
    held: VecDeque<Held>,
    /// The innermost prompt as the first input was held: one that opened since takes none of
    /// it.
    prompt: Option<PromptId>,
    /// What `C-g` stopped, dropping what was held, since the host last asked.
    cancelled: Option<Cancelled>,
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
        if matches!(input, Held::Key(key, _) if is_cancel(key)) {
            let cancelled = match editor.crossing().map(|c| (c.redo(), c.left())) {
                Some((redo, left)) => {
                    editor.cancel_crossing();
                    editor.message(if redo {
                        "Redo cancelled"
                    } else {
                        "Undo cancelled"
                    });
                    Cancelled::Crossing { left }
                }
                None => {
                    editor.replace_exit();
                    Cancelled::Replace
                }
            };
            self.held_during_all.cancelled = Some(cancelled);
            // A close request stays: the person asked for it apart from what they typed.
            self.held_during_all
                .held
                .retain(|held| matches!(held, Held::Close));
            return Some(Effects {
                redraw: true,
                ..Effects::default()
            });
        }
        let held = &mut self.held_during_all;
        if held.held.is_empty() {
            held.prompt = editor.prompts().last().map(|prompt| prompt.id());
        }
        // A close request is never dropped, however many keys came before it.
        if held.held.len() < MOST_HELD || matches!(input, Held::Close) {
            held.held.push_back(input);
        }
        Some(Effects::default())
    }

    /// Runs the input held while work went on, once it has ended. Returns what the host should
    /// do.
    pub fn run_held_during_all(&mut self, editor: &mut Editor) -> Effects {
        let mut effects = Effects::default();
        if holds_input(editor) {
            return effects;
        }
        // A prompt that opened meanwhile, a question above all, is answered by no key typed
        // before it.
        let innermost = editor.prompts().last().map(|prompt| prompt.id());
        let held = &mut self.held_during_all;
        if innermost.is_some() && innermost != held.prompt && !held.held.is_empty() {
            let had_keys = held.held.iter().any(|input| !matches!(input, Held::Close));
            held.held.retain(|input| matches!(input, Held::Close));
            if had_keys {
                editor.message(DROPPED);
            }
        }
        let before = self.from_socket;
        while !holds_input(editor) {
            let Some(input) = self.held_during_all.held.pop_front() else {
                break;
            };
            effects = effects.union(match input {
                Held::Key(key, from_socket) => {
                    self.from_socket = from_socket;
                    self.handle_key(editor, key)
                }
                Held::Text {
                    text,
                    commit,
                    from_socket,
                } => {
                    self.from_socket = from_socket;
                    if commit {
                        self.commit_text(editor, &text)
                    } else {
                        self.insert_text(editor, &text)
                    }
                }
                Held::Close => self.close_requested(editor),
            });
        }
        self.from_socket = before;
        effects
    }

    /// What `C-g` stopped since the last call, dropping what was held: what the host holds goes
    /// too, and the agent whose command began an undo hears it (ADR 0119).
    pub fn take_cancelled(&mut self) -> Option<Cancelled> {
        self.held_during_all.cancelled.take()
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
